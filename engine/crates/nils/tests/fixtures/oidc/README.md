<!-- SPDX-License-Identifier: AGPL-3.0-only -->

A throwaway RSA key pair for the `oidc` tests of `nils serve`: the tests
sign tokens with `signing-key.pem` and the server verifies them against
`jwks.json`, which holds the public half as the issuer's JWKS document would.
Generated once with `openssl genpkey`, never used anywhere else, and not a
secret: anyone may sign a token with it, and no deployment trusts it.
