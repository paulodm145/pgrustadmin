# Autenticação

Login obrigatório. Senhas com Argon2id v19, parâmetros m=19456 KiB, t=2, p=1 e saída de 32 bytes; gerar salt aleatório independente para cada senha e armazenar a string PHC completa. Respostas de falha não revelam existência de usuário. Sessões expiráveis e revogáveis; logout revoga no servidor. Cada endpoint protegido valida sessão e autorização. Não registrar senhas/tokens.

Aceite: login válido cria sessão; inválido retorna erro genérico; sessão expirada/revogada é recusada; usuário não acessa recursos de outro por alteração de ID; logout invalida sessão.

Em aberto: token/cookie, duração/renovação, CSRF se cookie, rate limit e recuperação de senha.

Hashing e verificação são operações síncronas que consomem CPU/memória. Handlers async devem executá-las fora do worker Tokio quando a API de autenticação for implementada.
