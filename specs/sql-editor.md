# Editor SQL

Cada aba mantém ID, conteúdo, arquivo, dirty, conexão/database, execução, resultado, duração e erro.

Arquivos: criar/abrir/salvar/salvar como `.sql` UTF-8; isolamento por usuário; servidor controla paths e bloqueia traversal; alertar ao fechar aba dirty; conflito de gravação não pode sobrescrever silenciosamente.

Execução: assíncrona; mostrar conexão/database; limite de linhas/volume; timeout e concorrência; cancelamento; separar resultados antigos/atuais; erro seguro. `EXPLAIN ANALYZE` exige confirmação.

SQL do usuário é arbitrário por definição. Não filtrar por regex. SQL interno parametriza valores.

Aceite: testes de abas, dirty tracking, arquivos, autorização, execução, limite, timeout, cancelamento e erros.

## SQL-001 — Estado das abas

As abas vivem apenas na memória da TUI e somem ao sair; persistência de conteúdo vem com arquivos `.sql` (SQL-003). Cada aba tem ID numérico único na sessão (nunca reutilizado), SQL (vazio ao abrir), arquivo associado (nenhum ao abrir), estado dirty (falso ao abrir), conexão (ID e nome) e database. O vínculo com conexão/database é fixo na aba. Execução, resultado, duração e erro entram no estado da aba com SQL-004/SQL-005, quando passam a ter comportamento; SQL-001 não os modela.

No máximo 20 abas ficam abertas ao mesmo tempo; abrir a 21ª é recusado com mensagem, sem alterar as abas existentes. Abrir uma aba a torna ativa. Fechar a aba ativa ativa a aba seguinte (ou a anterior, se era a última); fechar a última aba volta à lista de conexões. Alternar entre abas é circular.

Na TUI, `s` na lista de databases abre nova aba ligada à conexão e ao database selecionados e mostra a seção SQL. Nas telas Overview e Connections, `s` mostra a seção SQL quando houver abas abertas. Na seção SQL, a barra de abas mostra cada aba com número, database, conexão e marcador de dirty, e destaca a ativa; `[`/`]` alternam a aba ativa, `x` fecha a aba ativa e Tab/Esc voltam à lista de conexões. Edição do SQL, dirty tracking e alerta ao fechar aba dirty pertencem a SQL-002.

## SQL-002 — Editor e dirty tracking

A seção SQL tem dois modos. No modo de comandos valem os atalhos de SQL-001; Enter ou `i` entra no modo de edição da aba ativa, em que todas as teclas editam o SQL, e Esc volta ao modo de comandos. O rodapé indica o modo atual.

O editor é próprio e mínimo, sem nova dependência: texto multilinha com cursor; setas, Home/End e PgUp/PgDn movem o cursor (esquerda/direita atravessam quebras de linha; cima/baixo limitam a coluna ao tamanho da linha); Backspace/Delete removem caracteres e unem linhas; Enter quebra a linha; Tab insere 4 espaços; texto colado (bracketed paste) é inserido na posição do cursor com quebras `\r\n`/`\r` normalizadas para `\n`. A área rola para manter o cursor visível. Não há undo, seleção nem realce de sintaxe. A coluna do cursor conta caracteres Unicode; caracteres de largura dupla podem desalinhar o cursor visualmente.

Cada aba guarda o conteúdo base do último estado salvo (vazio ao abrir; arquivos chegam em SQL-003). A aba está dirty quando o SQL atual difere do conteúdo base; desfazer as alterações manualmente limpa o estado.

Fechar (`x`) uma aba dirty pede confirmação (`y` fecha, `n`/Esc cancela). Sair da TUI (`q`, Esc ou Ctrl+C) com abas dirty pede confirmação informando quantas abas têm alterações não salvas; `y` sai, `n`/Esc cancela e Ctrl+C durante a confirmação sai imediatamente. Sem abas dirty, a saída continua imediata.

## SQL-003 — Arquivos SQL

Arquivos pertencem ao usuário autenticado; identificador inexistente ou de outro usuário responde `404`. Metadados ficam na tabela `sql_files` do banco de metadados (ID UUID, proprietário, nome, storage key UUID, tamanho em bytes, SHA-256 do conteúdo, timestamps); o conteúdo UTF-8 fica em `PGRUSTADMIN_SQL_FILES_DIR/<owner_id>/<storage_key>.sql`. O caminho em disco é derivado só de UUIDs gerados pelo servidor, nunca do nome, o que impede traversal. O diretório é obrigatório e deve existir; sem ele, apenas as rotas de arquivos respondem `503` (`file_storage_unavailable`).

Nome: 5 a 255 bytes, termina em `.sql`, sem `/`, `\`, caracteres de controle, espaço nas extremidades ou ponto inicial; único por usuário (`409` `file_name_conflict`). Conteúdo: até 1 MiB (1 048 576 bytes) em UTF-8; acima disso `413` (`file_too_large`). Entrada inválida responde `400` (`invalid_request`).

Endpoints sob `/api/v1/sql-files`, com `sha256` em hexadecimal minúsculo como versão do conteúdo:
- `GET /api/v1/sql-files?page&per_page`: lista por nome com a paginação de conexões; `{ "items": [{ "id", "name", "size_bytes", "sha256" }], "page", "per_page" }`.
- `POST /api/v1/sql-files` com `{ "name", "content" }`: cria; `201` com os metadados.
- `GET /api/v1/sql-files/{id}`: metadados e `content`. Conteúdo ausente ou com hash divergente do registrado responde `500`.
- `PUT /api/v1/sql-files/{id}` com `{ "content", "expected_sha256" }`: grava somente se o hash atual for `expected_sha256`; caso contrário `409` (`file_conflict`) sem alterar nada. `200` com os novos metadados.
- `DELETE /api/v1/sql-files/{id}`: remove metadados e conteúdo; `204`.

Cada gravação escreve o conteúdo em nova storage key e só então atualiza o registro condicionado ao hash esperado; se a atualização falhar, o novo conteúdo é removido; se tiver sucesso, o conteúdo anterior é removido. Gravações concorrentes não sobrescrevem silenciosamente.

Na TUI (seção SQL, modo de comandos), cada aba guarda o arquivo associado (ID, nome e hash salvo). `w` salva: aba com arquivo faz `PUT` com o hash salvo; aba sem arquivo pede um nome (salvar como). `W` sempre pede um nome e cria um arquivo novo, que passa a ser o arquivo da aba. Nome digitado sem `.sql` recebe o sufixo. Quebras de linha `\r\n`/`\r` do arquivo aberto são normalizadas para `\n` pelo editor e gravadas assim no próximo salvamento. Depois de salvar, o conteúdo base da aba passa a ser o conteúdo salvo (dirty limpo). Em `409` de conflito, a TUI oferece `o` sobrescrever (lê o hash atual do servidor e grava com ele, ainda sujeito a conflito), `a` salvar como outro nome ou Esc cancelar. `o` lista os arquivos (até 100, por nome); Enter abre o selecionado em nova aba ligada à conexão/database da aba ativa, com o arquivo associado e dirty limpo; Esc fecha a lista. Abrir exige uma aba ativa. Excluir arquivos pela TUI não faz parte de SQL-003.

## SQL-004 — Execução assíncrona

SQL do usuário é executado como enviado, sem análise nem filtro. Cada execução abre uma conexão nova ao database informado com host, porta, usuário, credencial e política TLS da conexão salva (timeout de conexão de CON-004), envia o texto inteiro pelo simple query protocol em autocommit e fecha a conexão; nada persiste entre execuções (`SET`, transações abertas, tabelas temporárias). Vários comandos no mesmo texto rodam em sequência; o primeiro erro interrompe os seguintes, como no PostgreSQL.

`POST /api/v1/connections/{id}/databases/{database}/executions` com `{ "sql" }` inicia a execução e responde `202` com `{ "id", "status": "running" }`. SQL vazio (só espaços) ou maior que 1 MiB responde `400` (`invalid_request`). Autorização, `404` de conexão, `503` de chave e validação de `{database}` seguem CAT-002. Cada usuário tem no máximo 5 execuções `running`; acima disso `429` (`execution_limit_reached`).

`GET /api/v1/executions/{id}` responde `200` com `{ "id", "connection_id", "database", "status" }`, em que `status` é `running`, `succeeded` ou `failed`. Em `succeeded` inclui `"result": { "statement_count", "rows_returned", "rows_affected" }`: quantidade de comandos concluídos e, do último comando, linhas devolvidas e linhas afetadas informadas pelo PostgreSQL. As linhas são contadas, não armazenadas: colunas, valores, limites de linhas/volume e duração pertencem a SQL-005. Em `failed` inclui `"error": { "code", "message" }`: para erro do PostgreSQL, o SQLSTATE e a mensagem principal do servidor (sem detail/hint); para falha de conexão ou interna, `code` `null` e mensagem genérica, sem texto do driver nem segredos. Execução inexistente ou de outro usuário responde `404`.

As execuções ficam somente na memória do servidor e se perdem ao reiniciar. Cada usuário mantém no máximo 50 execuções; ao iniciar uma nova além disso, as concluídas mais antigas são descartadas. Execuções em andamento não são descartadas. Timeout de execução e cancelamento pertencem a SQL-006.

Na TUI, `e` no modo de comandos executa o SQL da aba ativa na conexão/database da aba. Uma aba não inicia nova execução enquanto a anterior está `running`. A TUI consulta o status a cada 300 ms em tarefa própria, sem bloquear o loop nem outras chamadas à API, e mostra na aba o estado atual: em andamento, sucesso (comandos, linhas devolvidas e afetadas do último comando) ou erro (SQLSTATE e mensagem). Fechar a aba descarta o acompanhamento local; a execução no servidor continua até terminar. Confirmação de `EXPLAIN ANALYZE` não faz parte de SQL-004.
