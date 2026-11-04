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
