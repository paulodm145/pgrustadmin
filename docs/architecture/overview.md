# Arquitetura

## Componentes
- `apps/server`: Axum + Tokio, autenticação, autorização e orquestração.
- `apps/tui`: Ratatui + Crossterm, cliente HTTP.
- `crates/api-client`: cliente HTTP tipado.
- `crates/app-core`: tipos/regras compartilháveis somente quando necessário.
- `crates/postgres-driver`: acesso PostgreSQL via SQLx.
- `apps/web` e `apps/desktop`: clientes futuros.

## Fluxo
TUI → API → autenticação/autorização → caso de uso → metadados e/ou PostgreSQL administrado. API é a fronteira de confiança.

## Dados
Banco de metadados separado dos bancos administrados. Metadados de arquivos no banco; conteúdo em storage controlado pelo servidor. Arquivos isolados por usuário.

## SQL
Execução assíncrona, associada a usuário/conexão/database. Aplicar limites, timeout, concorrência e cancelamento. Não confundir resultados antigos com execução atual.

## Rede
Bind local `127.0.0.1` por padrão. Acesso remoto requer TLS e configuração deliberada.
