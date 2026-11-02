"""Generate/check Windows icon frames from the original RGBA artwork (Pillow)."""

import argparse
import ctypes
import struct
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets/brand_icon.png"
OUTPUT = ROOT / "assets/brand_icon.ico"
SIZES = (16, 20, 24, 32, 40, 48, 64, 96, 128, 256)


def check_executable(path):
    # Read PE resources as data; never execute the application during this check.
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.LoadLibraryExW.argtypes = (wintypes.LPCWSTR, wintypes.HANDLE, wintypes.DWORD)
    kernel.LoadLibraryExW.restype = wintypes.HMODULE
    kernel.FindResourceW.argtypes = (wintypes.HMODULE, ctypes.c_void_p, ctypes.c_void_p)
    kernel.FindResourceW.restype = ctypes.c_void_p
    kernel.SizeofResource.argtypes = (wintypes.HMODULE, ctypes.c_void_p)
    kernel.SizeofResource.restype = wintypes.DWORD
    kernel.LoadResource.argtypes = (wintypes.HMODULE, ctypes.c_void_p)
    kernel.LoadResource.restype = ctypes.c_void_p
    kernel.LockResource.argtypes = (ctypes.c_void_p,)
    kernel.LockResource.restype = ctypes.c_void_p
    kernel.FreeLibrary.argtypes = (wintypes.HMODULE,)
    module = kernel.LoadLibraryExW(str(path.resolve()), None, 2)
    if not module:
        raise ctypes.WinError(ctypes.get_last_error())

    def resource(kind, identity):
        name_buffer = ctypes.create_unicode_buffer(identity) if isinstance(identity, str) else None
        if name_buffer is not None:
            identity = ctypes.cast(name_buffer, ctypes.c_void_p)
        entry = kernel.FindResourceW(module, identity, kind)
        if not entry:
            raise ctypes.WinError(ctypes.get_last_error())
        size = kernel.SizeofResource(module, entry)
        data = kernel.LockResource(kernel.LoadResource(module, entry))
        if not data or not size:
            raise ctypes.WinError(ctypes.get_last_error())
        return ctypes.string_at(data, size)

    try:
        ico = OUTPUT.read_bytes()
        groups = []
        callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HMODULE, ctypes.c_void_p, ctypes.c_void_p, wintypes.LPARAM)

        @callback_type
        def collect_group(module, kind, name, parameter):
            groups.append(ctypes.wstring_at(name) if name > 65535 else name)
            return True

        kernel.EnumResourceNamesW.argtypes = (wintypes.HMODULE, ctypes.c_void_p, callback_type, wintypes.LPARAM)
        kernel.EnumResourceNamesW.restype = wintypes.BOOL
        if not kernel.EnumResourceNamesW(module, 14, collect_group, 0):
            raise ctypes.WinError(ctypes.get_last_error())
        matched = False
        for group_name in groups:
            group = resource(14, group_name)
            if group[:6] != ico[:6]:
                continue
            for index in range(len(SIZES)):
                entry = group[6 + index * 14 : 20 + index * 14]
                expected = ico[6 + index * 16 : 22 + index * 16]
                identity = struct.unpack_from("<H", entry, 12)[0]
                length, offset = struct.unpack_from("<II", expected, 8)
                # The resource compiler normalizes PNG icon planes from 0 to 1.
                if entry[:4] != expected[:4] or entry[6:12] != expected[6:12] or resource(3, identity) != ico[offset : offset + length]:
                    break
            else:
                matched = True
                break
        if not matched:
            raise SystemExit(f"No embedded icon group matches all source frames: {path}")
        print(f"Executable icon verified: {path}")
    finally:
        kernel.FreeLibrary(module)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Check without rewriting")
    parser.add_argument("--exe", type=Path, action="append", default=[], help="Also check a Windows executable's embedded frames")
    args = parser.parse_args()
    with Image.open(SOURCE) as source:
        image = source.convert("RGBA")
    if image.width != image.height or image.width < max(SIZES):
        raise SystemExit("Source artwork must be square and at least 256 pixels wide")
    if not args.check:
        image.save(OUTPUT, format="ICO", sizes=[(side, side) for side in SIZES])
    with Image.open(OUTPUT) as icon:
        if icon.ico.sizes() != {(side, side) for side in SIZES}:
            raise SystemExit("Windows icon is missing required sizes")
        for side in SIZES:
            actual = icon.ico.getimage((side, side)).convert("RGBA")
            expected = image.resize((side, side), Image.Resampling.LANCZOS)
            if actual.tobytes() != expected.tobytes():
                raise SystemExit(f"Icon frame {side} differs from source artwork; regenerate it")
    print("App icon verified: " + ", ".join(str(side) for side in SIZES) + " px")
    for executable in args.exe:
        check_executable(executable)


if __name__ == "__main__":
    main()
