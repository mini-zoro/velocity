#!/bin/sh
# Ed25519 update signatures over the exact SHA256SUMS bytes. Requires OpenSSL 3.
set -eu
umask 077
openssl=${OPENSSL:-openssl}

public_hex() {
    # Check the RFC 8410 SubjectPublicKeyInfo prefix before printing the raw key.
    der=$("$openssl" pkey -in "$1" -pubout -outform DER | od -An -v -tx1 | tr -d '[:space:]')
    raw=${der#302a300506032b6570032100}
    if [ "$der" = "$raw" ] || [ "${#raw}" -ne 64 ]; then
        printf '%s\n' 'Expected an Ed25519 private key.' >&2
        exit 1
    fi
    printf '%s\n' "$raw"
}

case "${1:-}:$#" in
    generate:2)
        (set -C; "$openssl" genpkey -algorithm ED25519 > "$2")
        public_hex "$2"
        ;;
    public-hex:2)
        public_hex "$2"
        ;;
    sign:4)
        public_hex "$2" >/dev/null
        "$openssl" pkeyutl -sign -rawin -inkey "$2" -in "$3" -out "$4"
        ;;
    verify:4)
        "$openssl" pkeyutl -verify -rawin -pubin -inkey "$2" -in "$3" -sigfile "$4"
        ;;
    *)
        printf '%s\n' \
            'Usage: sh scripts/update-signing.sh generate PRIVATE.pem' \
            '       sh scripts/update-signing.sh public-hex PRIVATE.pem' \
            '       sh scripts/update-signing.sh sign PRIVATE.pem SHA256SUMS SHA256SUMS.sig' \
            '       sh scripts/update-signing.sh verify PUBLIC.pem SHA256SUMS SHA256SUMS.sig' >&2
        exit 2
        ;;
esac
