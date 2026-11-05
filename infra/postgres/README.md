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
