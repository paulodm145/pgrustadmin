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

## CAT-003 — Tabelas e views

`GET /api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations` conecta ao database informado como em CAT-002 e lista relações do schema em ordem crescente por nome. Inclui tabelas (`table`), tabelas particionadas (`partitioned_table`), views (`view`), views materializadas (`materialized_view`) e foreign tables (`foreign_table`); exclui partições filhas, índices, sequências e tipos compostos. Lista todas as relações visíveis no catálogo, sem filtrar por privilégio do objeto; schema inexistente ou sem `USAGE` para a credencial responde lista vazia.

Recebe `page` (começa em 1) e `per_page` (padrão 25, máximo 100), como conexões. Resposta `200`: `{ "items": [{ "name": "...", "kind": "table" }], "page", "per_page" }`. Paginação inválida ou `{schema}` vazio, com byte nulo ou com mais de 63 bytes responde `400`. Validação de `{database}`, autorização e demais erros seguem CAT-002.

A TUI abre a lista com Enter sobre o schema selecionado, mostra nome e tipo, permite navegar, recarregar e trocar de página com `[`/`]` (100 itens por página), e volta à lista de schemas com Esc. CAT-003 não lista colunas nem índices.

## CAT-004 — Colunas e índices

Dois endpoints sob `/api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations/{relation}` leem a relação informada (dos tipos listados em CAT-003) conectando ao database como em CAT-002. Não há paginação: PostgreSQL limita relações a 1600 colunas e índices por relação são poucos. Relação inexistente, de tipo não listado em CAT-003 ou em schema sem `USAGE` responde lista vazia. `{relation}` segue a validação de identificador de `{schema}` (`400`); autorização e demais erros seguem CAT-002.

`GET .../columns` lista colunas não removidas em ordem de posição. Resposta `200`: `{ "items": [{ "name", "position", "data_type", "nullable", "default", "primary_key", "identity", "generated" }] }`. `data_type` é o tipo formatado pelo PostgreSQL (ex.: `character varying(255)`); `default` é a expressão default ou `null`; `primary_key` indica participação na chave primária; `identity` é `"always"`, `"by_default"` ou `null`; `generated` é `"stored"`, `"virtual"` ou `null` (colunas geradas têm `default` `null`).

`GET .../indexes` lista índices da relação em ordem crescente por nome. Resposta `200`: `{ "items": [{ "name", "method", "unique", "primary", "columns": ["..."] }] }`. `method` é o método de acesso (ex.: `btree`, `gin`); `columns` traz as colunas-chave ou expressões na ordem do índice, com modificadores como `DESC`, sem colunas `INCLUDE`. A definição SQL completa não é retornada.

A TUI abre os detalhes com Enter sobre a relação selecionada, mostra colunas (com seleção navegável) e índices da relação, permite recarregar e volta à lista de relações com Esc.
