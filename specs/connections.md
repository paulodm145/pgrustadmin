# Conexões PostgreSQL

CRUD autorizado por proprietário. Campos mínimos: nome, host, porta, database padrão, usuário, TLS e credencial. Senha nunca é retornada após gravação; credencial cifrada em repouso conforme ADR-0003. Teste de conexão com timeout e erro sanitizado. Não registrar connection string com senha.

Para CON-002, usar XChaCha20-Poly1305 via RustCrypto. A chave vem de `PGRUSTADMIN_CREDENTIAL_KEY` como Base64 padrão de 32 bytes aleatórios. O envelope é `0x01 || nonce(24 bytes) || ciphertext || tag(16 bytes)`. AAD é `pgrustadmin/connection-credential/v1\0 || owner_uuid || connection_uuid`, com UUIDs em bytes. A chave ausente/inválida bloqueia a operação sem fallback em texto puro; não bloqueia outras rotas do servidor. Backup/recuperação e rotação seguem ADR-0003.

Aceite: CRUD isolado por usuário; teste distingue sucesso/falha sem vazar segredo; timeout aplicado; integração testada com PostgreSQL real.

Aceite adicional para CON-002: texto puro só é recuperado com a mesma chave, UUID do proprietário e UUID da conexão; alteração no envelope, chave ou contexto falha fechada; versão desconhecida é rejeitada.

Em aberto: compartilhamento e opções TLS. Rotação de chave no MVP é operação manual de manutenção conforme ADR-0003.
