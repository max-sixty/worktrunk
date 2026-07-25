"""Report whether every executable in the Windows release zip is signed.

The signing chain breaks silently — v0.69.0 and v0.69.1 both shipped unsigned
under a green run, two different ways — so this reads the zip itself and
nothing but the published bytes can satisfy it.

It checks that a certificate is present, not that it chains to a trusted root:
SignPath is on a self-signed test certificate pending the project's OSS
application, which is also why the caller runs this with continue-on-error.

Usage: python .github/verify-windows-signature.py <zip>
"""

import struct
import sys
import zipfile


def certificate_table_size(pe: bytes) -> int:
    """Size of the PE's certificate table — data directory entry 4, zero when unsigned."""
    pe_header = struct.unpack_from("<I", pe, 0x3C)[0]
    optional_header = pe_header + 24
    magic = struct.unpack_from("<H", pe, optional_header)[0]
    # Data directories follow the optional header, whose size differs between
    # PE32 (magic 0x10b) and PE32+ (0x20b).
    data_directories = optional_header + (96 if magic == 0x10B else 112)
    _, size = struct.unpack_from("<II", pe, data_directories + 4 * 8)
    return size


archive = sys.argv[1]
unsigned = []
with zipfile.ZipFile(archive) as zf:
    executables = [name for name in zf.namelist() if name.endswith(".exe")]
    if not executables:
        print(f"::error::{archive} contains no .exe to verify")
        sys.exit(1)
    for name in executables:
        size = certificate_table_size(zf.read(name))
        print(f"{name}: certificate table {size} bytes")
        if size == 0:
            unsigned.append(name)

if unsigned:
    print(f"::error::unsigned in {archive}: {', '.join(unsigned)}")
    sys.exit(1)
print(f"{archive}: all {len(executables)} executables signed")
