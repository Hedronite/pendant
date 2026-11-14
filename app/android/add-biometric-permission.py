#!/usr/bin/env python3
"""Add android.permission.USE_BIOMETRIC to an APK nsh packaged, and re-sign it.

nsh 0.5.0 writes AndroidManifest.xml from a fixed template with no key for
other permissions, and BiometricPrompt refuses an app that does not declare
USE_BIOMETRIC. This step adds that one permission and changes nothing else:

1. In the compiled (binary XML) manifest, it appends the permission name to
   the string pool and inserts a copy of the INTERNET <uses-permission>
   element that names it, right after the INTERNET one.
2. It writes the APK again with that manifest, every other entry as it was,
   then `zipalign -P 16 -f 4` and `apksigner sign` with the same local debug
   key nsh signed with. SDK build-tools only; nothing is installed.
3. It checks the result: `aapt2 dump permissions` gains exactly USE_BIOMETRIC,
   `aapt2 dump xmltree` differs only by that element, and `apksigner verify`
   and `zipalign -c` pass.

Usage (on the build host, after `nsh package --target android`):
    app/android/add-biometric-permission.py app/.nsh/out/Pendant.apk
The APK is replaced in place. ANDROID_HOME names the SDK.
"""

import difflib
import os
import struct
import subprocess
import sys
import tempfile
import zipfile

PERMISSION = "android.permission.USE_BIOMETRIC"
TEMPLATE = "android.permission.INTERNET"

RES_XML = 0x0003
STRING_POOL = 0x0001
START_ELEMENT = 0x0102
END_ELEMENT = 0x0103
UTF8 = 0x100
TYPE_STRING = 0x03


def fail(message):
    sys.exit(f"add-biometric-permission: {message}")


def u16(data, at):
    return struct.unpack_from("<H", data, at)[0]


def u32(data, at):
    return struct.unpack_from("<I", data, at)[0]


def read_pool(data, at):
    """The string pool at `at`: its fields and decoded strings."""
    header = u16(data, at + 2)
    size = u32(data, at + 4)
    count, styles, flags, start, styles_start = struct.unpack_from("<IIIII", data, at + 8)
    if styles:
        fail("a manifest string pool with styles is not handled")
    offsets = struct.unpack_from(f"<{count}I", data, at + header)
    utf8 = bool(flags & UTF8)
    strings = []
    for offset in offsets:
        p = at + start + offset
        if utf8:
            p += 2 if data[p] & 0x80 else 1  # length in characters
            n = data[p]
            if n & 0x80:
                n = ((n & 0x7F) << 8) | data[p + 1]
                p += 2
            else:
                p += 1
            strings.append(data[p : p + n].decode("utf-8"))
        else:
            n = u16(data, p)
            p += 2
            if n & 0x8000:
                n = ((n & 0x7FFF) << 16) | u16(data, p)
                p += 2
            strings.append(data[p : p + 2 * n].decode("utf-16-le"))
    return {"header": header, "size": size, "count": count, "flags": flags, "start": start}, strings


def encode(text, utf8):
    if utf8:
        raw = text.encode("utf-8")
        if len(text) > 0x7F or len(raw) > 0x7F:
            fail("a long string is not handled")
        return bytes([len(text), len(raw)]) + raw + b"\0"
    raw = text.encode("utf-16-le")
    return struct.pack("<H", len(text)) + raw + b"\0\0"


def add_permission(manifest):
    data = bytearray(manifest)
    if u16(data, 0) != RES_XML:
        fail("not a binary XML manifest")
    pool_at = u16(data, 2)
    if u16(data, pool_at) != STRING_POOL:
        fail("no string pool after the XML header")
    pool, strings = read_pool(data, pool_at)
    if PERMISSION in strings:
        fail(f"{PERMISSION} is already in the manifest")
    try:
        uses = strings.index("uses-permission")
        template = strings.index(TEMPLATE)
    except ValueError:
        fail("no INTERNET <uses-permission> to copy")
    new_index = pool["count"]

    # The string pool, one string longer: a new offset entry, the string at
    # the end of the data, padded to four bytes.
    header, count, start = pool["header"], pool["count"], pool["start"]
    utf8 = bool(pool["flags"] & UTF8)
    offsets = list(struct.unpack_from(f"<{count}I", data, pool_at + header))
    # The original string bytes exactly, up to their unpadded end; the new
    # string goes after them.
    end = max(o + len(encode(text, utf8)) for o, text in zip(offsets, strings))
    string_data = bytes(data[pool_at + start : pool_at + start + end])
    offsets.append(len(string_data))
    string_data += encode(PERMISSION, utf8)
    string_data += b"\0" * (-len(string_data) % 4)
    new_start = header + 4 * len(offsets)
    pool_header = bytearray(data[pool_at : pool_at + header])
    new_size = new_start + len(string_data)
    struct.pack_into("<I", pool_header, 4, new_size)
    struct.pack_into("<I", pool_header, 8, len(offsets))
    struct.pack_into("<I", pool_header, 20, new_start)
    new_pool = bytes(pool_header) + struct.pack(f"<{len(offsets)}I", *offsets) + string_data

    rest = bytes(data[pool_at + pool["size"] :])

    # In the chunks after the pool: the INTERNET element and its end.
    at = 0
    inserted = None
    while at < len(rest):
        kind, chunk_header, size = struct.unpack_from("<HHI", rest, at)
        if kind == START_ELEMENT:
            name = u32(rest, at + 16 + 4)
            count_attrs = u16(rest, at + 16 + 12)
            first = at + 16 + u16(rest, at + 16 + 8)
            step = u16(rest, at + 16 + 10)
            values = [u32(rest, first + i * step + 8) for i in range(count_attrs)]
            if name == uses and template in values:
                end_at = at + size
                if u16(rest, end_at) != END_ELEMENT or u32(rest, end_at + 16 + 4) != uses:
                    fail("the INTERNET element is not followed by its end")
                end_size = u32(rest, end_at + 4)
                start_chunk = bytearray(rest[at:end_at])
                for i in range(count_attrs):
                    a = (first - at) + i * step
                    if u32(start_chunk, a + 8) == template:
                        struct.pack_into("<I", start_chunk, a + 8, new_index)
                        if start_chunk[a + 15] != TYPE_STRING:
                            fail("the name attribute is not a string")
                        struct.pack_into("<I", start_chunk, a + 16, new_index)
                copy = bytes(start_chunk) + bytes(rest[end_at : end_at + end_size])
                cut = end_at + end_size
                rest = rest[:cut] + copy + rest[cut:]
                inserted = len(copy)
                break
        at += size
    if inserted is None:
        fail("no INTERNET <uses-permission> element")

    out = bytearray(bytes(data[:pool_at]) + new_pool + rest)
    struct.pack_into("<I", out, 4, len(out))
    return bytes(out)


def tool(sdk, name):
    root = os.path.join(sdk, "build-tools")
    versions = sorted(v for v in os.listdir(root) if os.path.exists(os.path.join(root, v, name)))
    if not versions:
        fail(f"no build-tools with {name}")
    return os.path.join(root, versions[-1], name)


def run(*args, capture=False):
    result = subprocess.run(args, check=True, capture_output=capture, text=True)
    return result.stdout if capture else None


def main():
    if len(sys.argv) != 2:
        fail("usage: add-biometric-permission.py <apk>")
    apk = sys.argv[1]
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk:
        fail("set ANDROID_HOME")
    aapt2, zipalign, apksigner = (tool(sdk, n) for n in ("aapt2", "zipalign", "apksigner"))
    keystore = os.path.expanduser("~/.android/debug.keystore")

    before_permissions = run(aapt2, "dump", "permissions", apk, capture=True)
    before_tree = run(aapt2, "dump", "xmltree", "--file", "AndroidManifest.xml", apk, capture=True)

    with tempfile.TemporaryDirectory() as work:
        unaligned = os.path.join(work, "unaligned.apk")
        aligned = os.path.join(work, "aligned.apk")
        with zipfile.ZipFile(apk) as source, zipfile.ZipFile(unaligned, "w") as target:
            for info in source.infolist():
                if info.filename.startswith("META-INF/"):
                    continue  # the old signature; apksigner signs again
                body = source.read(info.filename)
                if info.filename == "AndroidManifest.xml":
                    body = add_permission(body)
                target.writestr(info, body, compress_type=info.compress_type)
        run(zipalign, "-P", "16", "-f", "4", unaligned, aligned)
        run(
            apksigner, "sign", "--ks", keystore, "--ks-pass", "pass:android",
            "--key-pass", "pass:android", "--ks-key-alias", "androiddebugkey", aligned,
        )
        run(apksigner, "verify", aligned)
        run(zipalign, "-c", "-P", "16", "4", aligned)
        after_permissions = run(aapt2, "dump", "permissions", aligned, capture=True)
        after_tree = run(aapt2, "dump", "xmltree", "--file", "AndroidManifest.xml", aligned, capture=True)

        diff = list(difflib.ndiff(before_tree.splitlines(), after_tree.splitlines()))
        added = [line[2:] for line in diff if line.startswith("+ ")]
        removed = [line[2:] for line in diff if line.startswith("- ")]
        # One element line and its one attribute line, nothing taken away.
        if removed or len(added) != 2 or PERMISSION not in "\n".join(added):
            fail("the manifest changed beyond the one permission:\n+ " + "\n+ ".join(added) + "\n- " + "\n- ".join(removed))
        gained = set(after_permissions.splitlines()) - set(before_permissions.splitlines())
        lost = set(before_permissions.splitlines()) - set(after_permissions.splitlines())
        if lost or gained != {f"uses-permission: name='{PERMISSION}'"}:
            fail(f"permissions changed unexpectedly: +{sorted(gained)} -{sorted(lost)}")
        os.replace(aligned, apk)

    print("aapt2 dump permissions, before:")
    print(before_permissions.rstrip())
    print("aapt2 dump permissions, after:")
    print(after_permissions.rstrip())
    print("aapt2 dump xmltree: the only change is")
    print("\n".join("  + " + line.strip() for line in added))


if __name__ == "__main__":
    main()
