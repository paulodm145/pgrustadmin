# API

Prefixo versionado sugerido `/api/v1`; JSON UTF-8; erros com código estável e mensagem segura; paginação; validação de entrada; sem stack traces ou segredos. Autenticação obrigatória exceto health/version sem dados sensíveis.

Recursos iniciais: health/version; auth login/logout/session; connections CRUD/test; catalog databases/schemas/tables/views/columns/indexes; SQL execute/status/result/cancel; SQL files list/create/read/update/delete.

Autenticação inicial: `POST /api/v1/auth/login` recebe `{ "email", "password" }` e retorna `200` com `{ "access_token", "token_type": "Bearer", "expires_in", "user_id" }`; credenciais inválidas retornam `401` com `{ "error": "invalid_credentials" }`, sem indicar se o email existe. `POST /api/v1/auth/logout` revoga o token Bearer atual e responde `204`, inclusive se ele já for inválido. `GET /api/v1/auth/session` retorna `{ "user_id" }` para sessão válida ou `401` com `{ "error": "unauthorized" }`. Endpoints protegidos validam o token no servidor.

Definir schemas, HTTP status, paginação e idempotência ao implementar cada recurso. Não inventar endpoints sem requisito.
