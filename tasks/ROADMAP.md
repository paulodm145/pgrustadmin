# Roadmap

- Fase 0: auditar scaffold; criar workspace Rust mínimo; revisar PostgreSQL de desenvolvimento; definir config/logs/erros; migrations de metadados; CI.
- Fase 1: usuários, Argon2id, login/logout, sessões, autorização, proteção de credenciais.
- Fase 2: CRUD/teste de conexões, TLS/timeouts, API.
- Fase 3: TUI, login, conexões e catálogo.
- Fase 4: abas SQL, arquivos, execução, resultados, limites, timeout/cancelamento.
- Fase 5: testes integração/segurança, docs operacionais e empacotamento Linux.
- Futuro: Web React/TS, Desktop Tauri 2, outros bancos após ADR.

Cada etapa deve seguir o fluxo de `docs/development/spec-driven-workflow.md`; o roadmap indica ordem macro, enquanto `tasks/initial-backlog.md` registra dependências e estado verificável.
