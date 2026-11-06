# Backlog inicial

Use `docs/development/spec-driven-workflow.md` ao trabalhar em qualquer item. Antes de implementar, vincule a especificação relevante e critérios verificáveis. Tarefas de auditoria/decisão não exigem código; registrar evidências e decisões no status.

| ID | Tarefa | Dependência | Status |
|---|---|---|---|
| PREP-001 | Auditar scaffold/workspace | — | Concluída: sem manifestos Cargo; documentação e diretórios iniciais presentes. Auditoria por inventário, sem abrir `specs/`. |
| PREP-006 | Criar workspace Rust mínimo | PREP-001 | Concluída — `specs/workspace-bootstrap.md`; `cargo fmt --all -- --check` e `cargo check --workspace` passaram. |
| PREP-002 | Revisar configuração PostgreSQL local existente | PREP-001 | Concluída — revisão estática; `docker compose -f infra/postgres/docker-compose.yml config --quiet` passou. |
| PREP-003 | Configuração, logs e erros | PREP-006 | Concluída — `specs/server-foundation.md`; `cargo fmt --all -- --check`, `cargo check --workspace` e `cargo clippy --workspace --all-targets -- -D warnings` passaram. |
| PREP-004 | Migrations/metadados iniciais | PREP-002, PREP-003, PREP-006, AUTH-001, ADR-0002 | Concluída — `specs/metadata-migrations.md`; `cargo run -p pgrustadmin-server -- migrate` aplicou v1/v2 e o catálogo confirmou constraints/versões. |
| PREP-005 | CI fmt/test/Clippy | PREP-006 | Concluída — workflow em `.github/workflows/ci.yml`; o resultado da primeira execução remota será registrado na PR de tarefa. |
| AUTH-001 | Modelo de usuário | PREP-003, ADR-0002 | Concluída — `specs/metadata-schema.md`; UUID v4, email trim/lowercase e tipos do usuário em `app-core`; fmt, check e Clippy passaram. |
| AUTH-002 | Argon2id | AUTH-001 | Concluída — `specs/authentication.md`; parâmetros Argon2id e hash PHC implementados; 2 testes unitários, fmt, check e Clippy passaram. |
| AUTH-004 | Modelo e armazenamento de sessões | AUTH-002, PREP-004 | Concluída — `specs/authentication.md`, `specs/metadata-schema.md`; migration v3 e operações SQLx para criar, validar e revogar sessões; fmt, testes, check, Clippy e aplicação local da migration passaram. |
| AUTH-003 | Login/logout | AUTH-002, AUTH-004 | Concluída — `specs/authentication.md`, `specs/api.md`; endpoints login/logout/sessão, falha genérica, Argon2id fora do worker Tokio; fmt, 6 testes, check, Clippy e smoke HTTP com PostgreSQL passaram. |
| CON-002 | Criptografia credenciais | ADR-0003 | Concluída — `specs/connections.md`; envelope XChaCha20-Poly1305 versionado, chave Base64 por ambiente, AAD proprietário/conexão, backup/perda/rotação definidos; 11 testes, fmt, check e Clippy passaram. |
| CON-001 | CRUD conexões | AUTH-004, CON-002 | Concluída — `specs/connections.md`, `specs/api.md`; migration v4, CRUD paginado e owner-filtered, segredo cifrado; 13 testes, fmt, check, Clippy, migration e smoke HTTP com dois usuários passaram. |
| AUTH-005 | Autorização por recurso | CON-001 | Concluída — teste HTTP/PostgreSQL com duas sessões confirma lista isolada e `404` cruzado em GET/PUT/DELETE; CI usa PostgreSQL efêmero; 14 testes, fmt, check e Clippy passaram. |
| CON-003 | Teste conexão | CON-001, AUTH-005 | Concluída — POST protegido busca credencial por owner, descriptografa em memória e testa PostgreSQL; integração confirma sucesso, falha sanitizada e 404 para outro usuário. |
| CON-004 | TLS/timeouts | CON-003 | Concluída — TLS obrigatório sem downgrade quando habilitado; timeout total de 5 s; erro do driver reduzido a `success: false`; teste unitário confirma modos TLS e integração valida conexão real. |
| API-001 | API base | PREP-003 | Pendente |
| API-002 | Cliente HTTP | API-001 | Pendente |
| TUI-001 | Loop eventos/layout | PREP-001 | Pendente |
| TUI-002 | Login TUI | API-002/AUTH-003 | Pendente |
| TUI-003 | Conexões TUI | TUI-002/CON-001 | Pendente |
| CAT-001 | Databases | TUI-003 | Pendente |
| CAT-002 | Schemas | CAT-001 | Pendente |
| CAT-003 | Tabelas/views | CAT-002 | Pendente |
| CAT-004 | Colunas/índices | CAT-003 | Pendente |
| SQL-001 | Estado das abas | TUI-001 | Pendente |
| SQL-002 | Editor/dirty tracking | SQL-001 | Pendente |
| SQL-003 | Arquivos SQL | SQL-002/API-001 | Pendente |
| SQL-004 | Execução assíncrona | CON-001 | Pendente |
| SQL-005 | Resultados/duração | SQL-004 | Pendente |
| SQL-006 | Timeout/cancelamento | SQL-004 | Pendente |
| TUI-004 | Temas | TUI-001 | Pendente |
| TEST-001 | Auth/autorização | AUTH-* | Pendente |
| TEST-002 | Conexões/segredos | CON-* | Pendente |
| TEST-003 | Catálogo | CAT-* | Pendente |
| TEST-004 | Editor/arquivos | SQL-001..003 | Pendente |
| TEST-005 | Execução/cancelamento | SQL-004..006 | Pendente |

## Decisões confirmadas

- ADR-0002: PostgreSQL dedicado para os metadados do produto.
- ADR-0003: variável de ambiente como fonte inicial da chave de criptografia de credenciais externas. Detalhes do envelope, ausência da chave, backup e rotação devem ser definidos antes de CON-002.

## Próxima tarefa

CON-004 está concluída com timeout e política TLS documentados e validados. Próxima tarefa: API-001, estabelecer a base compartilhada da API conforme a especificação. O runtime conecta ao banco de metadados sem aplicar migrations automaticamente.
