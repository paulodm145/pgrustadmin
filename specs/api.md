# API

Prefixo versionado sugerido `/api/v1`; JSON UTF-8; erros com código estável e mensagem segura; paginação; validação de entrada; sem stack traces ou segredos. Autenticação obrigatória exceto health/version sem dados sensíveis.

Recursos iniciais: health/version; auth login/logout/session; connections CRUD/test; catalog databases/schemas/tables/views/columns/indexes; SQL execute/status/result/cancel; SQL files list/create/read/update/delete.

API-001 define `GET /api/v1/health` como liveness (`200`, `{ "status": "ok" }`) e `GET /api/v1/version` (`200`, `{ "version": "<versão do pacote>" }`), sem autenticação e sem dados de infraestrutura. Rotas desconhecidas respondem `404` JSON `{ "error": "not_found" }`; método não suportado em rota conhecida responde `405` JSON `{ "error": "method_not_allowed" }`. A base não expõe detalhes internos.

API-002 disponibiliza cliente assíncrono reutilizável com base URL de `PGRUSTADMIN_API_BASE_URL` (padrão `http://127.0.0.1:3000`), timeout HTTP de 30 segundos e TLS Rustls para URLs HTTPS. Cliente tipado cobre health/version, login/logout/session e CRUD/lista/teste de conexões. O token Bearer fica encapsulado e zerado ao descarte; erros expõem status/código estável ou categoria de transporte/decodificação, nunca corpo arbitrário da resposta, URL com credenciais ou token.

CAT-001 a CAT-004 adicionam ao cliente tipado a leitura de databases da conexão autenticada, de schemas de um database, de tabelas/views paginadas de um schema e de colunas/índices de uma relação conforme `specs/catalog.md`.

TUI-001 inicializa e restaura o terminal mesmo quando o loop termina com erro, processa eventos sem bloquear o runtime assíncrono, redesenha em resize e oferece navegação por teclado com foco visível em layout que se adapta a terminais estreitos. Nenhuma seção ainda executa operações da API nesta tarefa.

Autenticação inicial: `POST /api/v1/auth/login` recebe `{ "email", "password" }` e retorna `200` com `{ "access_token", "token_type": "Bearer", "expires_in", "user_id" }`; credenciais inválidas retornam `401` com `{ "error": "invalid_credentials" }`, sem indicar se o email existe. `POST /api/v1/auth/logout` revoga o token Bearer atual e responde `204`, inclusive se ele já for inválido. `GET /api/v1/auth/session` retorna `{ "user_id" }` para sessão válida ou `401` com `{ "error": "unauthorized" }`. Endpoints protegidos validam o token no servidor.

Conexões seguem o contrato, paginação e erros seguros definidos em `specs/connections.md`. A API nunca aceita `owner_id` do cliente e não retorna credenciais. O catálogo PostgreSQL segue `specs/catalog.md` e usa credenciais salvas autorizadas no servidor.

Definir schemas, HTTP status, paginação e idempotência ao implementar cada recurso. Não inventar endpoints sem requisito.
