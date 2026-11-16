#!/usr/bin/env python3
"""Verify a read-only HFS+ image's app bytes, modes and Applications link without mounting.

This intentionally reads only the unfragmented HFS+ subset produced by makehybrid. Images
requiring an extents-overflow tree fail verification instead of silently skipping files.
"""
import hashlib
from pathlib import Path
import stat
import struct
import sys
import unicodedata


def u16(data, offset):
    return struct.unpack_from(">H", data, offset)[0]


def u32(data, offset):
    return struct.unpack_from(">I", data, offset)[0]


def u64(data, offset):
    return struct.unpack_from(">Q", data, offset)[0]


def main():
    image, app = map(Path, sys.argv[1:])
    data = image.read_bytes()
    assert data[:2] == b"ER", "missing Apple driver descriptor map"
    sector_size = u16(data, 2)
    assert sector_size >= 512
    partition_offset = None
    count = u32(data, sector_size + 4)
    assert 0 < count < 4096
    for i in range(1, count + 1):
        entry = i * sector_size
        assert data[entry:entry + 2] == b"PM", "invalid Apple partition map"
        if data[entry + 48:entry + 80].split(b"\0", 1)[0] == b"Apple_HFS":
            assert partition_offset is None, "multiple HFS partitions"
            partition_offset = u32(data, entry + 8) * sector_size
    assert partition_offset is not None, "missing HFS partition"
    header = data[partition_offset + 1024:partition_offset + 1536]
    assert header[:2] in [b"H+", b"HX"], "not an HFS+ volume"
    block_size = u32(header, 40)
    assert block_size >= 512 and block_size & (block_size - 1) == 0

    def fork(record, offset):
        logical_size = u64(record, offset)
        chunks = []
        remaining = logical_size
        for i in range(8):
            start = u32(record, offset + 16 + i * 8)
            blocks = u32(record, offset + 20 + i * 8)
            length = min(remaining, blocks * block_size)
            begin = partition_offset + start * block_size
            assert begin + length <= len(data), "extent exceeds image"
            chunks.append(data[begin:begin + length])
            remaining -= length
        assert remaining == 0, "extents-overflow image is unsupported by this verifier"
        return b"".join(chunks)

    catalog = fork(header, 272)
    node_size = u16(catalog, 32)
    node = u32(catalog, 24)
    assert node_size >= 512 and node_size & (node_size - 1) == 0
    folders = {}
    files = []
    visited = set()
    while node:
        assert node not in visited, "catalog leaf cycle"
        visited.add(node)
        leaf = catalog[node * node_size:(node + 1) * node_size]
        assert len(leaf) == node_size and leaf[8] == 255, "invalid catalog leaf"
        records = u16(leaf, 10)
        assert records < node_size // 2
        for i in range(records):
            start = u16(leaf, node_size - (i + 1) * 2)
            end = u16(leaf, node_size - (i + 2) * 2)
            record = leaf[start:end]
            key_length = u16(record, 0)
            parent = u32(record, 2)
            characters = u16(record, 6)
            assert 8 + characters * 2 <= key_length + 2 <= len(record)
            name = unicodedata.normalize("NFC", record[8:8 + characters * 2].decode("utf-16be"))
            value = record[2 + key_length:]
            kind = u16(value, 0)
            if kind == 1:
                folders[u32(value, 8)] = (parent, name)
            elif kind == 2:
                files.append((parent, name, u16(value, 42), fork(value, 88)))
        node = u32(leaf, 0)

    def relative(parent, name):
        parts = [name]
        visited_parents = set()
        while parent != 2:
            assert parent not in visited_parents, "catalog parent cycle"
            visited_parents.add(parent)
            parent, name = folders[parent]
            parts.append(name)
        assert all(p and p not in [".", ".."] and "/" not in p for p in parts)
        return "/".join(reversed(parts))

    actual = {}
    for parent, name, mode, content in files:
        path = relative(parent, name)
        assert path not in actual, "duplicate HFS path"
        actual[path] = (mode, content)
    mode, target = actual.pop("Applications")
    assert stat.S_ISLNK(mode) and target == b"/Applications", "Applications is not the expected symlink"
    expected = {}
    for path in app.rglob("*"):
        if path.is_file() or path.is_symlink():
            relative_path = unicodedata.normalize("NFC", "Velocity.app/" + str(path.relative_to(app)))
            expected[relative_path] = path
    assert set(actual) == set(expected), "HFS file set differs: " + repr(set(actual) ^ set(expected))
    for name, path in expected.items():
        mode, content = actual[name]
        if path.is_symlink():
            assert stat.S_ISLNK(mode) and content.decode() == str(path.readlink()), name
        else:
            assert stat.S_ISREG(mode), name
            assert mode & 0o777 == path.stat().st_mode & 0o777, name + " mode"
            assert hashlib.sha256(content).digest() == hashlib.sha256(path.read_bytes()).digest(), name + " content"
    print("DMG HFS+ read-back passed: {} app files, modes, Applications -> /Applications".format(len(expected)))


if __name__ == "__main__":
    main()
