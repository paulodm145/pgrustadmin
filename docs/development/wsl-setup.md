# WSL setup

Pré-requisitos: WSL 2, Rust stable/Cargo via rustup, Git, Docker (Desktop com integração WSL ou Engine), editor com Rust Analyzer.

Verifique `rustc --version`, `cargo --version`, `rustup show`. Instale componentes com `rustup component add rustfmt clippy`.

Prefira manter o repositório no filesystem Linux (`~/src/pgrustadmin`) em vez de `/mnt/c`. Use Compose em `infra/postgres`. Rode `cargo fmt --all -- --check`, `cargo test --workspace` e `cargo clippy --workspace --all-targets -- -D warnings`. Registre comandos não executados.
