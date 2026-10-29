# Cloudflare Access test keys

**These keys are for tests only. They are not secrets and must never be used for anything
real.** The private key is committed on purpose, so the unit tests in `src/remote/access.rs`
can sign tokens the way Cloudflare Access does, and serve the matching public keys.

- `test-key.pem`: a 2048-bit RSA private key (PKCS#8, PEM), made with
  `openssl genrsa 2048` and converted with `openssl pkcs8 -topk8 -nocrypt`.
- `jwks.json`: the public key as a JWK set (`n`, `e`, `kid` = `test-key-2026`), the same shape
  as the set Cloudflare publishes at `https://<team>/cdn-cgi/access/certs`.
