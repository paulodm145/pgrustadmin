# Autenticação

Login obrigatório. Senhas com Argon2id v19, parâmetros m=19456 KiB, t=2, p=1 e saída de 32 bytes; gerar salt aleatório independente para cada senha e armazenar a string PHC completa. Respostas de falha não revelam existência de usuário. Sessões expiráveis e revogáveis; logout revoga no servidor. Cada endpoint protegido valida sessão e autorização. Não registrar senhas/tokens.

Aceite: login válido cria sessão; inválido retorna erro genérico; sessão expirada/revogada é recusada; usuário não acessa recursos de outro por alteração de ID; logout invalida sessão.

Decisão inicial: clientes enviam um token opaco no cabeçalho `Authorization: Bearer`. O servidor gera um UUID v4 com 122 bits aleatórios, entrega o token uma vez e persiste somente seu hash SHA-256. A sessão expira em 12 horas, sem renovação no MVP. O token Bearer evita CSRF baseado em cookies; clientes web devem proteger a sessão contra XSS. Rate limit e recuperação de senha permanecem em aberto; o provisionamento é definido em USR-001.

Hashing e verificação são operações síncronas que consomem CPU/memória. Handlers async devem executá-las fora do worker Tokio quando a API de autenticação for implementada.

## USR-001 — Criar usuário pela CLI

`pgrustadmin-server user create --email <email>` cria um usuário ativo no banco de metadados de `PGRUSTADMIN_METADATA_DATABASE_URL` (migrations já aplicadas). O email é normalizado como no login (espaços externos removidos, minúsculas) e deve ter formato mínimo: até 254 caracteres, exatamente um `@` com partes não vazias, sem espaços e com ponto no domínio.

No modo interativo a senha é pedida duas vezes no terminal, sem eco, e as duas digitações devem coincidir. Com `--password-stdin` a senha é a primeira linha da entrada padrão (sem `\n`/`\r\n` final), sem confirmação, para automação sem expor a senha em argumentos ou histórico. A senha deve ter de 12 a 1024 caracteres Unicode, sem exigência de classes de caracteres. O hash usa Argon2id com os parâmetros desta especificação, calculado fora do worker Tokio.

Em sucesso, o comando imprime o ID do usuário criado e termina com código 0. Email já cadastrado, argumentos ou entradas inválidas e falhas de banco terminam com código diferente de zero e mensagem curta, sem exibir a senha, o hash nem a URL do banco. A senha lida é zerada ao sair de escopo.
