# Test vectors

- `rfc8032-ed25519.txt`: the five Ed25519 test vectors of RFC 8032 section
  7.1 (TEST 1, 2, 3, 1024 and SHA(abc)), extracted from the RFC's text: name,
  public key, message and signature in hex, `-` for the empty message.
- `wycheproof-ed25519.json`: `testvectors_v1/ed25519_test.json` of
  [C2SP/wycheproof](https://github.com/C2SP/wycheproof) at commit
  `a3d5d1423f2592d6646836a45a21a1b209da9f4d`, SHA-256
  `ea80ffebac085e21cb9d3e903e7fa2b448d96702da5b31f061d0b83b430b74b3`,
  licensed Apache-2.0.
