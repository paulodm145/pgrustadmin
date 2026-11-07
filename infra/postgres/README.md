# PostgreSQL local

```bash
docker compose up -d
docker compose ps
docker compose logs -f postgres
```

Host `127.0.0.1`, porta `55432`, database `pgrustadmin_dev`, usuário `pgrustadmin`. Senha local descartável definida no Compose.

`docker compose down` preserva volume. `docker compose down -v` apaga os dados locais.
O seed em `init/` roda apenas na primeira inicialização do volume.

Para aplicar migrations ao banco de metadados, defina `PGRUSTADMIN_METADATA_DATABASE_URL` para este banco e execute, na raiz do repositório:

```bash
cargo run -p pgrustadmin-server -- migrate
```

Não registre nem compartilhe o valor da URL; ela contém credenciais. O servidor normal não aplica migrations automaticamente.

Após aplicar as migrations, o servidor usa a mesma variável para autenticação e atende apenas em `127.0.0.1:3000` por padrão:

```bash
export PGRUSTADMIN_METADATA_DATABASE_URL='postgresql://pgrustadmin:pgrustadmin_dev@127.0.0.1:55432/pgrustadmin_dev'
cargo run -p pgrustadmin-server
```

O endpoint de login autentica usuários ativos já cadastrados em `users`; criação/provisionamento de usuários ainda não tem comando ou endpoint definido.

Antes de cadastrar conexões após a implementação de CON-001, gere uma chave de 32 bytes com `openssl rand -base64 32` e forneça-a em `PGRUSTADMIN_CREDENTIAL_KEY`. Guarde a mesma chave fora do PostgreSQL e junto ao plano de backup: trocar ou perder a chave torna credenciais já cifradas irrecuperáveis. Não gere uma chave nova em cada inicialização.

Arquivos `.sql` salvos pela API ficam fora do PostgreSQL, num diretório controlado pelo servidor. Crie-o e informe-o antes de iniciar o servidor; sem a variável, só as rotas de arquivos respondem `503`:

```bash
mkdir -p ~/.local/share/pgrustadmin/sql-files
export PGRUSTADMIN_SQL_FILES_DIR=~/.local/share/pgrustadmin/sql-files
```

Inclua esse diretório no backup junto com o banco de metadados: os metadados referenciam o conteúdo por storage key, e um sem o outro não restaura os arquivos. O teste de integração usa um diretório temporário próprio e não precisa da variável.

O teste de autorização precisa de um banco descartável. Crie-o uma vez:

```bash
docker compose -f infra/postgres/docker-compose.yml exec -T postgres createdb -U pgrustadmin pgrustadmin_test
export PGRUSTADMIN_TEST_DATABASE_URL='postgresql://pgrustadmin:pgrustadmin_dev@127.0.0.1:55432/pgrustadmin_test'
export PGRUSTADMIN_TEST_CONNECTION_PASSWORD='pgrustadmin_dev'
export PGRUSTADMIN_TEST_CONNECTION_PORT=55432
export PGRUSTADMIN_CREDENTIAL_KEY='KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio='
cargo test --workspace
```

A chave acima é somente uma fixture pública de teste. Não a use para conexões reais.
