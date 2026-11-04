# Autenticação

Login obrigatório. Senhas com Argon2id. Respostas de falha não revelam existência de usuário. Sessões expiráveis e revogáveis; logout revoga no servidor. Cada endpoint protegido valida sessão e autorização. Não registrar senhas/tokens.

Aceite: login válido cria sessão; inválido retorna erro genérico; sessão expirada/revogada é recusada; usuário não acessa recursos de outro por alteração de ID; logout invalida sessão.

Em aberto: token/cookie, duração/renovação, CSRF se cookie, rate limit e recuperação de senha.
