import copy
import gzip
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest

spec = importlib.util.spec_from_file_location(
    "verifier", Path(__file__).with_name("verify-publication.py")
)
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)


class PublicationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.home = Path(cls.directory.name)
        cls.home.chmod(0o700)
        cls.gpg = ["gpg", "--homedir", str(cls.home), "--batch", "--pinentry-mode", "loopback"]
        subprocess.run(
            cls.gpg + ["--passphrase", "", "--quick-generate-key",
                       "Publication Test <test@example.invalid>", "ed25519", "sign", "0"],
            check=True, capture_output=True,
        )
        cls.keyring = cls.home / "trusted.gpg"
        cls.keyring.write_bytes(subprocess.check_output(cls.gpg + ["--export"]))

    @classmethod
    def tearDownClass(cls):
        subprocess.run(["gpgconf", "--homedir", str(cls.home), "--kill", "all"], check=True)
        cls.directory.cleanup()

    def setUp(self):
        self.objects = {}
        self.requests = []
        self.payload = b"synthetic deb archive"
        self.artifact = {
            "key": "ppa/test/pool/main/t/test/test_1_arm64.deb",
            "sha256": hashlib.sha256(self.payload).hexdigest(),
            "size": len(self.payload),
            "package-name": "test",
            "package-version": "1",
            "architecture": "arm64",
        }
        self.report = {"dry-run": False, "manifests": [
            {"kind": "deb", "artifacts": [self.artifact]}
        ]}
        test = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                test.requests.append((self.path, dict(self.headers)))
                if self.path not in test.objects:
                    self.send_error(404)
                    return
                data = test.objects[self.path]
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.send_header("CF-Cache-Status", "HIT")
                self.send_header("Age", "60")
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *_):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.base = f"http://127.0.0.1:{self.server.server_port}"
        self.snapshot()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def snapshot(self, version="1", gzip_content=None, by_hash=True):
        artifact = self.artifact
        packages = (
            f"Package: test\nVersion: {version}\nArchitecture: arm64\n"
            f"Filename: pool/main/t/test/test_1_arm64.deb\n"
            f"SHA256: {artifact['sha256']}\nSize: {artifact['size']}\n\n"
        ).encode()
        indexes = {
            "main/binary-arm64/Packages": packages,
            "main/binary-arm64/Packages.gz": gzip.compress(
                gzip_content if gzip_content is not None else packages, mtime=0
            ),
            "main/binary-arm64/Release": b"Architecture: arm64\n",
        }
        root = "/ppa/test/dists/preview/"
        release = (
            f"Acquire-By-Hash: {'yes' if by_hash else 'no'}\n"
            "Components: main\nArchitectures: arm64\nSHA256:\n"
        )
        for name, data in indexes.items():
            digest = hashlib.sha256(data).hexdigest()
            release += f" {digest} {len(data)} {name}\n"
            self.objects[root + name] = data
            self.objects[root + str(Path(name).parent) + "/by-hash/SHA256/" + digest] = data
        release += "SHA512:\n"
        for name, data in indexes.items():
            digest = hashlib.sha512(data).hexdigest()
            release += f" {digest} {len(data)} {name}\n"
            self.objects[root + str(Path(name).parent) + "/by-hash/SHA512/" + digest] = data
        self.objects[root + "Release"] = release.encode()
        path = self.home / "Release"
        path.write_text(release)
        for name, arguments in (
            ("InRelease", ["--clearsign"]),
            ("Release.gpg", ["--armor", "--detach-sign"]),
        ):
            destination = self.home / name
            subprocess.run(
                self.gpg + ["--yes", "--output", str(destination)] + arguments + [str(path)],
                check=True, capture_output=True,
            )
            self.objects[root + name] = destination.read_bytes()
        self.objects["/" + artifact["key"]] = self.payload

    def verify(self, report=None):
        return verifier.verify(self.base, report or self.report, "ppa/test", "preview", self.keyring)

    def test_canonical_urls_and_signed_snapshot_pass(self):
        self.assertEqual(self.verify(), (1, 3))
        for path, headers in self.requests:
            self.assertNotIn("?", path)
            self.assertEqual(headers["User-Agent"], "genmeta-xtask-release/0.2")
            self.assertNotIn("Cache-Control", headers)
            self.assertNotIn("Pragma", headers)

    def test_stale_canonical_gzip_fails_even_if_query_would_succeed(self):
        key = "/ppa/test/dists/preview/main/binary-arm64/Packages.gz"
        self.objects[key + "?release=1"] = self.objects[key]
        self.objects[key] = b"old cached gzip"
        with self.assertRaisesRegex(ValueError, "CF-Cache-Status=HIT.*expected.*received"):
            self.verify()
        self.assertFalse(any("?" in path for path, _ in self.requests))

    def test_coherent_old_snapshot_is_not_success(self):
        self.snapshot(version="0")
        with self.assertRaisesRegex(ValueError, "does not contain the published artifact"):
            self.verify()

    def test_missing_by_hash_fails(self):
        for key in list(self.objects):
            if "/by-hash/" in key:
                del self.objects[key]
        with self.assertRaises(verifier.urllib.error.HTTPError):
            self.verify()

    def test_changed_payload_fails(self):
        self.objects["/" + self.artifact["key"]] = b"different"
        with self.assertRaisesRegex(ValueError, "expected.*received"):
            self.verify()

    def test_signed_but_inconsistent_gzip_fails(self):
        self.snapshot(gzip_content=b"different packages\n")
        with self.assertRaisesRegex(ValueError, "raw and gzip indexes disagree"):
            self.verify()

    def test_missing_acquire_by_hash_fails(self):
        self.snapshot(by_hash=False)
        with self.assertRaisesRegex(ValueError, "Acquire-By-Hash"):
            self.verify()

    def test_invalid_signature_fails(self):
        self.objects["/ppa/test/dists/preview/InRelease"] += b"corruption"
        # Change the signed cleartext itself; trailing data alone can be tolerated by gpgv.
        self.objects["/ppa/test/dists/preview/InRelease"] = self.objects[
            "/ppa/test/dists/preview/InRelease"
        ].replace(b"Components: main", b"Components: evil")
        with self.assertRaises(subprocess.CalledProcessError):
            self.verify()

    def test_all_package_must_appear_in_every_architecture(self):
        report = copy.deepcopy(self.report)
        report["manifests"][0]["artifacts"][0]["architecture"] = "all"
        with self.assertRaisesRegex(ValueError, "does not contain the published artifact"):
            self.verify(report)

    def test_dry_run_report_fails(self):
        with self.assertRaisesRegex(ValueError, "dry-run"):
            self.verify({"dry-run": True})


if __name__ == "__main__":
    unittest.main()
