# Catálogo PostgreSQL

O catálogo pertence à conexão salva e só pode ser lido pelo proprietário autenticado. A API usa a configuração e credencial cifrada da conexão; nunca aceita host, usuário, senha ou `owner_id` fornecidos na requisição. Identificador de conexão inexistente ou de outro usuário responde `404`.

## CAT-001 — Databases

`GET /api/v1/connections/{id}/databases` conecta ao database padrão da conexão e lista `pg_database` em ordem crescente por nome. Inclui somente databases que aceitam conexão, não são templates e para os quais a credencial salva tem privilégio `CONNECT`. Resposta `200`: `{ "items": [{ "name": "..." }] }`.

A operação aplica a política TLS e timeout de conexão existente. Chave de credencial ausente/inválida retorna `503` (`credential_encryption_unavailable`); falha/timeout no banco administrado retorna `502` (`catalog_unavailable`), sem mensagem do driver. Não há paginação: a fonte é o catálogo local de databases do servidor PostgreSQL.

A TUI permite abrir a lista de databases da conexão selecionada, navegar e voltar à lista de conexões. CAT-001 não executa comandos sobre databases.

## CAT-002 — Schemas

`GET /api/v1/connections/{id}/databases/{database}/schemas` conecta ao database informado usando host, porta, usuário, credencial e política TLS da conexão salva, e lista `pg_namespace` em ordem crescente por nome. Inclui somente schemas em que a credencial tem privilégio `USAGE`. Inclui `pg_catalog` e `information_schema`; exclui schemas internos `pg_toast*` e temporários `pg_temp_*`/`pg_toast_temp_*`. Resposta `200`: `{ "items": [{ "name": "..." }] }`.

`{database}` é um segmento de caminho codificado em URL; nome vazio, com byte nulo ou com mais de 63 bytes responde `400` (`bad_request`). Database inexistente, sem `CONNECT` ou que recusa conexão é indistinguível de falha do banco administrado e responde `502` (`catalog_unavailable`), sem mensagem do driver. Autorização, `404`, `503`, timeout e ausência de paginação seguem CAT-001.

A TUI abre a lista de schemas com Enter sobre o database selecionado, permite navegar e recarregar, e volta à lista de databases com Esc. CAT-002 não lista objetos dentro dos schemas.
