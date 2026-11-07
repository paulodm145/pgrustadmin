# Catálogo PostgreSQL

O catálogo pertence à conexão salva e só pode ser lido pelo proprietário autenticado. A API usa a configuração e credencial cifrada da conexão; nunca aceita host, usuário, senha ou `owner_id` fornecidos na requisição. Identificador de conexão inexistente ou de outro usuário responde `404`.

## CAT-001 — Databases

`GET /api/v1/connections/{id}/databases` conecta ao database padrão da conexão e lista `pg_database` em ordem crescente por nome. Inclui somente databases que aceitam conexão, não são templates e para os quais a credencial salva tem privilégio `CONNECT`. Resposta `200`: `{ "items": [{ "name": "..." }] }`.

A operação aplica a política TLS e timeout de conexão existente. Chave de credencial ausente/inválida retorna `503` (`credential_encryption_unavailable`); falha/timeout no banco administrado retorna `502` (`catalog_unavailable`), sem mensagem do driver. Não há paginação: a fonte é o catálogo local de databases do servidor PostgreSQL.

A TUI permite abrir a lista de databases da conexão selecionada, navegar e voltar à lista de conexões. CAT-001 não executa comandos sobre databases.
