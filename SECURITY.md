# Security

NILS will hold pseudonymized medical imaging metadata and, in its linkage store, the identities behind it, so reports are taken seriously now, before there is code to run.

**Report a vulnerability** to admin@kineuro.se, or through "Report a vulnerability" under this repository's Security tab (private vulnerability reporting is on). Please do not open a public issue for it. We answer within a week, fix confirmed issues as fast as we can, and credit you in the release notes if you wish.

**In scope now:** the repository itself (its workflows, templates and the `cla-signatures` branch) and the design record in `docs/decisions/`, whose custody decisions (D13, C38, D24) welcome scrutiny before they are built. When the engine exists, this file lists what it protects and how, in the same form as [Bifrost's](https://github.com/kineuro/bifrost/blob/main/SECURITY.md).

**Out of scope:** the operating system, reverse proxy, storage and identity services you run NILS behind, and denial of service through legitimate large ingests.
