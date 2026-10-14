#!/usr/bin/env python3
"""Read-only public-key/transport probe; sends NAT tests, no account credentials."""
import argparse
import base64
import ctypes
import ctypes.util
import hashlib
import json
import socket


def varint(number):
    out = bytearray()
    while number > 127:
        out.append((number & 127) | 128)
        number >>= 7
    out.append(number)
    return bytes(out)


def field(number, data):
    return varint(number * 8 + 2) + varint(len(data)) + data


def fields(data):
    position = 0

    def integer():
        nonlocal position
        value = shift = 0
        while True:
            byte = data[position]
            position += 1
            value |= (byte & 127) << shift
            if byte < 128:
                return value
            shift += 7
            assert shift <= 63

    result = {}
    while position < len(data):
        tag = integer()
        if tag % 8 == 2:
            length = integer()
            value = data[position:position + length]
            assert len(value) == length
            position += length
        else:
            assert tag % 8 == 0
            value = integer()
        result.setdefault(tag // 8, []).append(value)
    return result


def exact(stream, count):
    data = bytearray()
    while len(data) < count:
        part = stream.recv(count - len(data))
        assert part, "Unexpected EOF"
        data.extend(part)
    return bytes(data)


def receive(stream):
    first = exact(stream, 1)
    header = first + exact(stream, first[0] & 3)
    length = int.from_bytes(header, "little") >> 2
    assert length <= 1048576
    return exact(stream, length)


def send(stream, payload):
    length = len(payload)
    width = next(i for i in range(1, 5) if length < (1 << (8 * i - 2)))
    stream.sendall(((length << 2) | (width - 1)).to_bytes(width, "little") + payload)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--address", required=True)
    parser.add_argument("--public-key", required=True)
    args = parser.parse_args()
    host, port = args.address.rsplit(":", 1)
    address = (host, int(port))
    sodium = ctypes.CDLL(ctypes.util.find_library("sodium"))
    assert sodium.sodium_init() >= 0

    def verify(signed, public):
        result = ctypes.create_string_buffer(len(signed))
        count = ctypes.c_ulonglong()
        assert sodium.crypto_sign_open(result, ctypes.byref(count), signed, ctypes.c_ulonglong(len(signed)), public) == 0
        return result.raw[:count.value]

    nat = field(20, b"")
    with socket.create_connection(address, timeout=8) as stream:
        send(stream, nat)
        assert 21 in fields(receive(stream)), "Plain NAT route changed"

    with socket.create_connection(address, timeout=8) as stream:
        greeting = fields(fields(receive(stream))[25][0])
        server = verify(greeting[1][0], base64.b64decode(args.public_key))
        assert len(server) == 32
        assert greeting[2] == [1]
        signed_parameters = verify(greeting[3][0], base64.b64decode(args.public_key))
        assert signed_parameters == b"rdkx-params" + field(1, server) + b"\x10\x01"
        public = ctypes.create_string_buffer(32)
        private = ctypes.create_string_buffer(32)
        assert sodium.crypto_box_keypair(public, private) == 0
        key = ctypes.create_string_buffer(32)
        sodium.randombytes_buf(key, 32)
        sealed = ctypes.create_string_buffer(48)
        assert sodium.crypto_box_easy(sealed, key, ctypes.c_ulonglong(32), bytes(24), server, private) == 0
        send(stream, field(25, field(1, public.raw) + field(1, sealed.raw) + b"\x10\x01"))

        def directional(direction):
            transcript = b"rdkx-spl" + bytes([direction]) + (1).to_bytes(4, "little") * 2 + public.raw + server
            return hashlib.blake2b(transcript, key=key.raw, digest_size=32).digest()

        for sequence in [1, 2]:
            nonce = sequence.to_bytes(8, "little") + bytes(16)
            encrypted = ctypes.create_string_buffer(len(nat) + 16)
            assert sodium.crypto_secretbox_easy(encrypted, nat, ctypes.c_ulonglong(len(nat)), nonce, directional(1)) == 0
            send(stream, encrypted.raw)
            reply = receive(stream)
            decrypted = ctypes.create_string_buffer(len(reply) - 16)
            assert sodium.crypto_secretbox_open_easy(decrypted, reply, ctypes.c_ulonglong(len(reply)), nonce, directional(2)) == 0
            assert 21 in fields(decrypted.raw)
            # hbbs closes after a NAT request, so one request tests the real routing path.
            break
    print(json.dumps({"address": args.address, "plain_nat": "passed", "signature": "passed", "signed_version": 1,
                      "encrypted_nat": "passed", "account_credentials_sent": False}))


if __name__ == "__main__":
    main()
