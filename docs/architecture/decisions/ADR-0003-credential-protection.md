# ADR-0003 — Proteção de credenciais
- Status: Aceito
- Data: 2026-11-02 (detalhado em 2026-11-05)

Senhas de login: hash Argon2id, nunca criptografia reversível. Credenciais dos bancos externos: criptografia autenticada AEAD em repouso. Chave mestra fora do banco, fornecida por secret manager, arquivo protegido ou mecanismo do SO.

Para o MVP, a chave mestra vem da variável `PGRUSTADMIN_CREDENTIAL_KEY`, em Base64 padrão de exatamente 32 bytes aleatórios. O servidor usa XChaCha20-Poly1305 com nonce aleatório de 24 bytes e envelope binário versionado: byte de versão `0x01`, nonce, ciphertext e tag de autenticação de 16 bytes. AAD é o prefixo `pgrustadmin/connection-credential/v1\0` seguido dos UUIDs binários do proprietário e da conexão. O vínculo impede que um envelope seja movido entre usuários ou conexões sem falhar a autenticação.

Chave ausente ou inválida faz a operação de credencial falhar sem fallback em texto puro; não impede inicialização do servidor para rotas que não usam credenciais externas. Backup do PostgreSQL e cópia da chave são independentes e ambos são necessários para recuperar os dados. Perder a chave torna as credenciais irrecuperáveis.

O MVP usa uma chave ativa sem identificador de chave no envelope. Rotação exige janela de manutenção: parar gravações, manter a chave antiga protegida, recriptografar todas as credenciais com a nova chave, validar o resultado e então atualizar o ambiente. Backups antigos ainda exigem a chave correspondente até expirarem. A aplicação não faz rotação automática.

Usar a biblioteca mantida `chacha20poly1305` do RustCrypto e não criar criptografia própria. A variante XChaCha oferece nonce estendido de 24 bytes, que simplifica geração aleatória para cada envelope; não há especificação final da IETF para essa variante, e o envelope é interno ao produto. Nunca registrar chaves, credenciais, plaintext ou envelopes.
