#!/usr/bin/env python3
"""Verify an APT publication through canonical public URLs (Python >= 3.11)."""

import argparse
import gzip
import hashlib
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request


def fetch(base, key):
    # No query parameters, request cache directives or cache-busting headers:
    # this must observe the same CDN objects that ordinary APT clients receive.
    url = base.rstrip("/") + "/" + urllib.parse.quote(key, safe="/~")
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            data = response.read()
            cache = ", ".join(
                f"{name}={response.headers[name]}"
                for name in ("CF-Cache-Status", "Age", "Cache-Control")
                if name in response.headers
            )
    except urllib.error.HTTPError as error:
        error.close()
        raise
    return data, f"{url} ({cache})"


def checked_fetch(base, key, digest, size):
    data, source = fetch(base, key)
    actual = hashlib.sha256(data).hexdigest()
    if len(data) != size or actual != digest:
        raise ValueError(
            f"{source}: expected {size} bytes / SHA256 {digest}; "
            f"received {len(data)} bytes / SHA256 {actual}"
        )
    return data


def fields(text):
    result = {}
    name = None
    for line in text.splitlines():
        if line.startswith((" ", "\t")) and name:
            result[name] += "\n" + line.strip()
        elif ":" in line:
            name, value = line.split(":", 1)
            result[name] = value.strip()
    return result


def verify(base, report, prefix, suite, keyring):
    if report.get("dry-run") is not False:
        raise ValueError("refusing to verify a dry-run report")
    artifacts = [
        artifact
        for manifest in report["manifests"]
        if manifest["kind"] == "deb"
        for artifact in manifest["artifacts"]
    ]
    if not artifacts:
        raise ValueError("publication report has no DEB artifacts")
    root = f"{prefix.strip('/')}/dists/{suite}"
    signed, _ = fetch(base, f"{root}/InRelease")
    release, _ = fetch(base, f"{root}/Release")
    signature, _ = fetch(base, f"{root}/Release.gpg")
    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        for name, content in (
            ("InRelease", signed), ("Release", release), ("Release.gpg", signature)
        ):
            (directory / name).write_bytes(content)
        command = ["gpgv", "--keyring", str(Path(keyring).resolve())]
        subprocess.run(
            command + ["--output", str(directory / "verified"), str(directory / "InRelease")],
            check=True, capture_output=True,
        )
        subprocess.run(
            command + [str(directory / "Release.gpg"), str(directory / "Release")],
            check=True, capture_output=True,
        )
        if (directory / "verified").read_bytes() != release:
            raise ValueError("canonical InRelease and Release describe different snapshots")
    metadata = fields(release.decode())
    if metadata.get("Acquire-By-Hash") != "yes":
        raise ValueError("signed Release must enable Acquire-By-Hash: yes")
    indexes = {}
    for row in metadata["SHA256"].strip().splitlines():
        digest, size, relative = row.split()
        path = PurePosixPath(relative)
        if path.is_absolute() or ".." in path.parts or "by-hash" in path.parts:
            raise ValueError(f"unexpected Release checksum path: {relative}")
        if path.name not in ("Packages", "Packages.gz", "Release"):
            raise ValueError(f"unexpected index in Release: {relative}")
        # In particular reject the old partially generated top-level Release.
        if len(path.parts) != 3 or not path.parts[1].startswith("binary-"):
            raise ValueError(f"unexpected index layout: {relative}")
        data = checked_fetch(base, f"{root}/{relative}", digest, int(size))
        hashed = f"{root}/{path.parent}/by-hash/SHA256/{digest}"
        checked_fetch(base, hashed, digest, int(size))
        indexes[relative] = data
    expected_indexes = {
        f"{component}/binary-{arch}/{name}"
        for component in metadata["Components"].split()
        for arch in metadata["Architectures"].split()
        for name in ("Packages", "Packages.gz", "Release")
    }
    if set(indexes) != expected_indexes:
        raise ValueError("Release is missing required architecture/component indexes")
    sha512_paths = set()
    for row in metadata["SHA512"].strip().splitlines():
        digest, size, relative = row.split()
        if relative not in indexes:
            raise ValueError(f"unexpected SHA512 index: {relative}")
        data = indexes[relative]
        if len(data) != int(size) or hashlib.sha512(data).hexdigest() != digest:
            raise ValueError(f"signed SHA256 and SHA512 checksums disagree: {relative}")
        path = PurePosixPath(relative)
        checked_fetch(
            base, f"{root}/{path.parent}/by-hash/SHA512/{digest}",
            hashlib.sha256(data).hexdigest(), len(data),
        )
        sha512_paths.add(relative)
    if sha512_paths != expected_indexes:
        raise ValueError("Release is missing required SHA512 indexes")
    for relative, content in indexes.items():
        if relative.endswith("/Packages.gz"):
            if gzip.decompress(content) != indexes[relative.removesuffix(".gz")]:
                raise ValueError(f"canonical raw and gzip indexes disagree: {relative}")
    for artifact in artifacts:
        key = artifact["key"]
        if not key.startswith(prefix.strip("/") + "/pool/"):
            raise ValueError(f"artifact is outside APT repository: {key}")
        filename = key[len(prefix.strip("/")) + 1:]
        component = PurePosixPath(filename).parts[1]
        architectures = (
            metadata["Architectures"].split()
            if artifact["architecture"] == "all" else [artifact["architecture"]]
        )
        for arch in architectures:
            relative = f"{component}/binary-{arch}/Packages"
            entries = [fields(block) for block in indexes[relative].decode().split("\n\n")]
            if not any(
                entry.get("Package") == artifact["package-name"]
                and entry.get("Version") == artifact["package-version"]
                and entry.get("Architecture") == artifact["architecture"]
                and entry.get("Filename") == filename
                and entry.get("SHA256") == artifact["sha256"]
                and entry.get("Size") == str(artifact["size"])
                for entry in entries
            ):
                raise ValueError(f"{relative} does not contain the published artifact {key}")
        checked_fetch(base, key, artifact["sha256"], artifact["size"])
    return len(artifacts), len(indexes)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--suite", required=True)
    parser.add_argument("--keyring", required=True)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--retry-interval", type=float, default=15)
    args = parser.parse_args()
    url = urllib.parse.urlsplit(args.base_url)
    if url.scheme not in ("http", "https") or not url.netloc or url.query or url.fragment:
        parser.error("--base-url must be an HTTP(S) URL without query or fragment")
    if args.timeout < 0 or args.retry_interval <= 0:
        parser.error("timeout must be nonnegative and retry-interval must be positive")
    report = tomllib.loads(args.report.read_text())
    deadline = time.monotonic() + args.timeout
    while True:
        try:
            artifacts, indexes = verify(
                args.base_url, report, args.prefix, args.suite, args.keyring
            )
            print(f"Public APT URLs verified: {artifacts} artifacts, {indexes} indexes + by-hash")
            return 0
        except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
            detail = (
                error.stderr.decode(errors="replace").strip()
                if isinstance(error, subprocess.CalledProcessError) else str(error)
            )
            print(f"Public APT verification failed: {detail}", file=sys.stderr, flush=True)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return 1
            time.sleep(min(args.retry_interval, remaining))


if __name__ == "__main__":
    sys.exit(main())
