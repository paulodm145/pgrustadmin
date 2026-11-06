# PGRustAdmin

Gerenciador PostgreSQL com TUI, API/servidor e interfaces futuras Web e Desktop.

## Estado
Workspace Rust preparado. O servidor inclui migrations explícitas de metadados, hashing Argon2id e endpoints iniciais de login/logout/sessão; os demais recursos do MVP continuam pendentes. A presença de uma especificação não significa que uma funcionalidade esteja implementada.

## MVP
- PostgreSQL como único banco administrado.
- Servidor Axum + Tokio; TUI Ratatui + Crossterm.
- Autenticação, sessões, autorização e conexões protegidas.
- Navegação inicial do catálogo PostgreSQL.
- Editor SQL com abas, arquivos `.sql`, execução, resultados, limites, timeout e cancelamento.

Web (React + TypeScript), Desktop (Tauri 2), MySQL e SQLite ficam para etapas futuras.

Comece por `AGENTS.md`, `tasks/START-HERE.md` e `docs/development/spec-driven-workflow.md`. Consulte visão, arquitetura e a especificação relevante antes de cada implementação.

Validação:
```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`cargo test --workspace` inclui teste HTTP/PostgreSQL e exige `PGRUSTADMIN_TEST_DATABASE_URL` apontando para uma base descartável; veja `infra/postgres/README.md`.

CI executa esses comandos em push e pull request via `.github/workflows/ci.yml`.
