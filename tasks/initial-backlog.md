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
| API-001 | API base | PREP-003 | Concluída — health/version públicos e respostas JSON com códigos estáveis para rota/método ausentes; testes cobrindo os quatro contratos. |
| API-002 | Cliente HTTP | API-001 | Concluída — cliente reqwest reutilizável, URL/env local, timeout, TLS Rustls, auth e conexões tipados; token e senha protegidos, erros sem body arbitrário; 3 testes de cliente e suíte workspace passaram. |
| TUI-001 | Loop eventos/layout | PREP-001 | Concluída — terminal restaura em erro/saída, loop Tokio não bloqueante, resize/redraw e layout adaptável com foco/navegação; 2 testes de estado/layout passaram. |
| TUI-002 | Login TUI | API-002/AUTH-003 | Concluída — formulário com senha mascarada, navegação por teclado, login e verificação de sessão em tarefa assíncrona; erros seguros, senha zeroizada e cancelamento ao sair; 3 testes TUI passaram. |
| TUI-003 | Conexões TUI | TUI-002/CON-001 | Concluída — listagem paginada, criação/edição (senha vazia preserva credencial), teste e exclusão confirmada; chamadas assíncronas, mensagens sem segredo; 5 testes TUI passaram. |
| CAT-001 | Databases | TUI-003 | Concluída — `specs/catalog.md`; `GET /api/v1/connections/{id}/databases` owner-scoped (404 para outro usuário, 502 `catalog_unavailable` sem mensagem do driver), filtra templates/sem `CONNECT`, ordenado por nome; cliente tipado e lista navegável na TUI com retorno às conexões; teste de integração do servidor e 6 testes TUI passaram. |
| CAT-002 | Schemas | CAT-001 | Concluída — `specs/catalog.md`; `GET /api/v1/connections/{id}/databases/{database}/schemas` owner-scoped conecta ao database escolhido com a credencial salva, lista schemas com `USAGE` (inclui `pg_catalog`/`information_schema`, exclui `pg_toast*`/`pg_temp_*`), `400` para nome inválido e `502` para database indisponível; cliente tipado com nome codificado no caminho; TUI abre schemas com Enter e volta com Esc; integração do servidor, 3 testes de cliente e 7 testes TUI passaram. |
| CAT-003 | Tabelas/views | CAT-002 | Concluída — `specs/catalog.md`; `GET .../schemas/{schema}/relations` paginado (padrão 25, máx. 100) lista tabelas, particionadas, views, views materializadas e foreign tables com `kind`, sem partições filhas nem filtro por privilégio do objeto; schema inexistente/sem `USAGE` retorna lista vazia; paginação e validação de identificador compartilhadas no servidor; TUI abre relações com Enter e pagina com `[`/`]`; integração do servidor, 3 testes de cliente e 8 testes TUI passaram. |
| CAT-004 | Colunas/índices | CAT-003 | Concluída — `specs/catalog.md`; `GET .../relations/{relation}/columns` (posição, tipo formatado, nullable, default, PK, identity, generated) e `.../indexes` (método, unique, primary, colunas-chave/expressões com `DESC`/`NULLS`), sem paginação; relação inexistente retorna lista vazia; TUI mostra colunas e índices com Enter sobre a relação; integração do servidor com fixture de identity/gerada/índices, 3 testes de cliente e 10 testes TUI (incl. renderização) passaram. |
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

CAT-001 a CAT-004 estão concluídas: o catálogo navega de databases a colunas e índices pela API e TUI. Próximas tarefas sem dependência pendente: SQL-001 (estado das abas), SQL-004 (execução assíncrona) e TUI-004 (temas).
