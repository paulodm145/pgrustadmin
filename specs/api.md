# API

Prefixo versionado sugerido `/api/v1`; JSON UTF-8; erros com código estável e mensagem segura; paginação; validação de entrada; sem stack traces ou segredos. Autenticação obrigatória exceto health/version sem dados sensíveis.

Recursos iniciais: health/version; auth login/logout/session; connections CRUD/test; catalog databases/schemas/tables/views/columns/indexes; SQL execute/status/result/cancel; SQL files list/create/read/update/delete.

Definir schemas, HTTP status, paginação e idempotência ao implementar cada recurso. Não inventar endpoints sem requisito.
