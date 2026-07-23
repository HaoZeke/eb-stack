# eOn 2.17.10 (foss-2026.1) + module rgpot

- `e/eOn/eOn-2.17.10-foss-2026.1.eb` — no patches; full meson test suite; deps include module `rgpot` + `nlohmann_json`
- `r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb` — fat build: RPC server (`potserv`), Eigen backend, nanobind Python bindings, unit + example tests (#26480 fat-build review)
- `n/nanobind/nanobind-2.13.0-GCCcore-15.2.0.eb` — Python binding companion for fat rgpot (first easyconfig; none upstream)
- Companions (PR): CapnProto 1.4.0, quill 11.1.0, readcon-core 0.13.1 (GCCcore-15.2.0)
