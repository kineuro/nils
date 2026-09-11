# NILS

Neuroimaging Intelligent Linked System. NILS reads DICOM into a registry, classifies the scans, answers queries over the registry and exports BIDS.

This repository holds the engine: one Rust binary, `nils`, with a command line and an HTTP API over a SQLite or PostgreSQL registry, and the setup wizard that installs NILS.

Pre-alpha.

## Install

```sh
curl -fsSL https://nils.kineuro.se/get | sh
```

## Documentation

https://kineuro.se/nils/docs/

## Related repositories

- [nils-desk](https://github.com/kineuro/nils-desk): the web application
- [nils-assistant](https://github.com/kineuro/nils-assistant): the assistant
- [kvasir](https://github.com/kineuro/kvasir): the model gateway

## License

AGPL-3.0-only. The contracts in `contracts/` are Apache-2.0. See [CONTRIBUTING.md](CONTRIBUTING.md).
