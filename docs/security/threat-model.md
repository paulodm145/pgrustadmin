# Modelo inicial de ameaças

## Ativos
Senhas/sessões, credenciais externas, arquivos SQL, dados acessíveis nos bancos, chave mestra e logs.

## Ameaças
Acesso cruzado entre usuários; IDs/paths adulterados; interceptação de sessão; path traversal; vazamento em logs/erros/backups; consultas demoradas e resultados volumosos.

## Controles
Autorização server-side em cada operação; sessões expiráveis/revogáveis; Argon2id para senha de login; AEAD para credenciais externas e chave fora do banco; TLS remoto; bind local por padrão; paths gerados/controlados pelo servidor; limites, timeout, concorrência e cancelamento; logs sem segredos; backup e recuperação de chave.

SQL arbitrário é feature deliberada. Não prometer sandbox inexistente nem filtrar comandos por regex. A consulta usa permissões do usuário PostgreSQL configurado. Mostrar conexão/database ativos. `EXPLAIN ANALYZE` exige confirmação porque executa a consulta.

Pendências: TLS/certificados, rate limiting, rotação/recuperação da chave, auditoria e retenção de logs, backup/restore.
