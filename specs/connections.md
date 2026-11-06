# Conexões PostgreSQL

CRUD autorizado por proprietário. Campos mínimos: nome, host, porta, database padrão, usuário, TLS e credencial. Senha nunca é retornada após gravação; credencial cifrada em repouso conforme ADR-0003. Teste de conexão com timeout e erro sanitizado. Não registrar connection string com senha.

CON-001 usa `POST /api/v1/connections`, `GET /api/v1/connections`, `GET /api/v1/connections/{id}`, `PUT /api/v1/connections/{id}` e `DELETE /api/v1/connections/{id}`. Cada registro tem UUID e nome único por proprietário. O `user_id` vem da sessão validada e nunca do corpo da requisição. Toda leitura/alteração/remoção por ID inclui `owner_id` na consulta; ID inexistente ou pertencente a outro usuário responde `404`.

Entrada contém `name`, `host`, `port`, `database`, `username`, `tls_enabled` e, na criação, `password`. `tls_enabled` assume `true` se omitido. Atualização substitui os campos de configuração e preserva a senha cifrada se `password` for omitida; valor presente substitui a senha. Campos textuais exceto senha são trim; nome tem 1–100 caracteres, host até 255, host/database/username são obrigatórios; porta é 1–65535. Nome é único por proprietário com comparação exata/case-sensitive. Não há normalização nem teste de DNS no CRUD.

Respostas incluem ID, nome, host, porta, database, username e `tls_enabled`; nunca incluem senha, envelope ou hash. Lista recebe `page` (começa em 1) e `per_page` (padrão 25, máximo 100) e responde `{ "items", "page", "per_page" }`. `POST` responde `201`, `GET`/`PUT` `200`, `DELETE` `204`; entrada inválida `400`, sessão ausente/inválida `401`, nome duplicado no mesmo usuário `409`, recurso ausente/não pertencente `404`, chave de credenciais ausente/inválida `503`, outra falha interna `500`, sempre com código de erro estável e mensagem segura.

Para CON-002, usar XChaCha20-Poly1305 via RustCrypto. A chave vem de `PGRUSTADMIN_CREDENTIAL_KEY` como Base64 padrão de 32 bytes aleatórios. O envelope é `0x01 || nonce(24 bytes) || ciphertext || tag(16 bytes)`. AAD é `pgrustadmin/connection-credential/v1\0 || owner_uuid || connection_uuid`, com UUIDs em bytes. A chave ausente/inválida bloqueia a operação sem fallback em texto puro; não bloqueia outras rotas do servidor. Backup/recuperação e rotação seguem ADR-0003.

Aceite CON-001: CRUD isolado por usuário, ownership verificado no servidor e no SQL, senha nunca retornada nem persistida em texto puro, paginação e conflito de nome documentados.

`POST /api/v1/connections/{id}/test` testa a credencial armazenada, exige autenticação e ownership e responde `200` com `{ "success": true|false }`. Falhas de conexão não retornam a mensagem original do driver. O endpoint descriptografa a credencial em memória e a consulta ao registro é limitada ao proprietário.

CON-004: `tls_enabled=true` usa `sslmode=require`, sem fallback para conexão sem TLS; `false` desativa TLS explicitamente. A conexão de teste tem limite fixo de 5 segundos. Erro de rede, autenticação, TLS ou timeout retorna `success: false` sem detalhes do driver. `require` exige criptografia e só valida a CA quando uma CA raiz está configurada pelo ambiente; validação do nome do servidor e configuração de certificados não fazem parte deste MVP.

Aceite CON-003: teste de conexão distingue sucesso/falha sem vazar segredo e integra com PostgreSQL real. Aceite CON-004: TLS e timeout aplicados e erros sanitizados.

Aceite adicional para CON-002: texto puro só é recuperado com a mesma chave, UUID do proprietário e UUID da conexão; alteração no envelope, chave ou contexto falha fechada; versão desconhecida é rejeitada.

Em aberto: compartilhamento e opções TLS além do booleano inicial; certificados e modos de validação ficam para CON-004. Rotação de chave no MVP é operação manual de manutenção conforme ADR-0003.
