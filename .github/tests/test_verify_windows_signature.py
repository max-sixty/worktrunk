"""Exercise the release checker against the actual bytes in a zip."""
import pathlib
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "verify-windows-signature.py"


def executable(magic, offset=512, size=16):
    pe = bytearray(528)
    pe[:2] = b"MZ"
    struct.pack_into("<I", pe, 0x3C, 128)
    pe[128:132] = b"PE\0\0"
    struct.pack_into("<H", pe, 152, magic)
    directories = 152 + (96 if magic == 0x10B else 112)
    struct.pack_into("<II", pe, directories + 4 * 8, offset, size)
    # WIN_CERTIFICATE header and a synthetic payload, not a trusted certificate.
    struct.pack_into("<IHH", pe, 512, 16, 0x200, 2)
    pe[520:528] = b"fixture!"
    return pe


class CertificateTableBounds(unittest.TestCase):
    def check_archive(self, payload):
        with tempfile.TemporaryDirectory() as directory:
            archive = pathlib.Path(directory) / "release.zip"
            with zipfile.ZipFile(archive, "w") as zf:
                zf.writestr("wt.exe", payload)
            return subprocess.run([sys.executable, str(SCRIPT), str(archive)], capture_output=True, text=True)

    def test_present_table(self):
        for magic in (0x10B, 0x20B):
            with self.subTest(magic=magic):
                result = self.check_archive(executable(magic))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_or_truncated_table(self):
        for magic in (0x10B, 0x20B):
            for offset, size in ((0, 16), (512, 0), (512, 17), (529, 16)):
                with self.subTest(magic=magic, offset=offset, size=size):
                    result = self.check_archive(executable(magic, offset, size))
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertNotIn("all 1 executables signed", result.stdout)


if __name__ == "__main__":
    unittest.main()
