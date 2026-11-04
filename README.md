# PGRustAdmin

Gerenciador PostgreSQL com TUI, API/servidor e interfaces futuras Web e Desktop.

## Estado
Workspace Rust preparado, com inicialização básica do servidor e migration inicial de metadados. As funcionalidades do MVP continuam pendentes; a presença de uma especificação não significa que estejam implementadas.

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

CI executa esses comandos em push e pull request via `.github/workflows/ci.yml`.
