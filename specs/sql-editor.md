# Editor SQL

Cada aba mantém ID, conteúdo, arquivo, dirty, conexão/database, execução, resultado, duração e erro.

Arquivos: criar/abrir/salvar/salvar como `.sql` UTF-8; isolamento por usuário; servidor controla paths e bloqueia traversal; alertar ao fechar aba dirty; conflito de gravação não pode sobrescrever silenciosamente.

Execução: assíncrona; mostrar conexão/database; limite de linhas/volume; timeout e concorrência; cancelamento; separar resultados antigos/atuais; erro seguro. `EXPLAIN ANALYZE` exige confirmação.

SQL do usuário é arbitrário por definição. Não filtrar por regex. SQL interno parametriza valores.

Aceite: testes de abas, dirty tracking, arquivos, autorização, execução, limite, timeout, cancelamento e erros.

## SQL-001 — Estado das abas

As abas vivem apenas na memória da TUI e somem ao sair; persistência de conteúdo vem com arquivos `.sql` (SQL-003). Cada aba tem ID numérico único na sessão (nunca reutilizado), SQL (vazio ao abrir), arquivo associado (nenhum ao abrir), estado dirty (falso ao abrir), conexão (ID e nome) e database. O vínculo com conexão/database é fixo na aba. Execução, resultado, duração e erro entram no estado da aba com SQL-004/SQL-005, quando passam a ter comportamento; SQL-001 não os modela.

No máximo 20 abas ficam abertas ao mesmo tempo; abrir a 21ª é recusado com mensagem, sem alterar as abas existentes. Abrir uma aba a torna ativa. Fechar a aba ativa ativa a aba seguinte (ou a anterior, se era a última); fechar a última aba volta à lista de conexões. Alternar entre abas é circular.

Na TUI, `s` na lista de databases abre nova aba ligada à conexão e ao database selecionados e mostra a seção SQL. Nas telas Overview e Connections, `s` mostra a seção SQL quando houver abas abertas. Na seção SQL, a barra de abas mostra cada aba com número, database, conexão e marcador de dirty, e destaca a ativa; `[`/`]` alternam a aba ativa, `x` fecha a aba ativa e Esc volta à lista de conexões (Tab volta até SQL-005, que o usa para alternar o foco). Edição do SQL, dirty tracking e alerta ao fechar aba dirty pertencem a SQL-002.

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

`POST /api/v1/connections/{id}/databases/{database}/executions` com `{ "sql" }` inicia a execução e responde `202` com `{ "id", "status": "running" }`. SQL vazio (só espaços) ou maior que 1 MiB responde `400` (`invalid_request`). Autorização, `404` de conexão, `503` de chave e validação de `{database}` seguem CAT-002. Cada usuário tem no máximo 5 execuções `running`; acima disso `429` (`execution_limit_reached`). O servidor inteiro tem no máximo `PGRUSTADMIN_MAX_CONCURRENT_EXECUTIONS` execuções `running` somando todos os usuários (inteiro de 1 a 1000, padrão 20; valor inválido impede o servidor de iniciar); acima disso `429` (`server_execution_limit_reached`). O limite do usuário é verificado primeiro. O limite global vale por processo do servidor.

`GET /api/v1/executions/{id}` responde `200` com `{ "id", "connection_id", "database", "status" }`, em que `status` é `running`, `succeeded` ou `failed`. Em `succeeded` inclui `"result": { "statement_count", "rows_returned", "rows_affected" }`: quantidade de comandos concluídos e, do último comando, linhas devolvidas e linhas afetadas informadas pelo PostgreSQL. Colunas, valores, limites de linhas/volume e duração são definidos em SQL-005. Em `failed` inclui `"error": { "code", "message" }`: para erro do PostgreSQL, o SQLSTATE e a mensagem principal do servidor (sem detail/hint); para falha de conexão ou interna, `code` `null` e mensagem genérica, sem texto do driver nem segredos. Execução inexistente ou de outro usuário responde `404`.

As execuções ficam somente na memória do servidor e se perdem ao reiniciar. Cada usuário mantém no máximo 50 execuções; ao iniciar uma nova além disso, as concluídas mais antigas são descartadas. Execuções em andamento não são descartadas. Timeout de execução e cancelamento pertencem a SQL-006.

Na TUI, `e` no modo de comandos executa o SQL da aba ativa na conexão/database da aba. Uma aba não inicia nova execução enquanto a anterior está `running`. A TUI consulta o status a cada 300 ms em tarefa própria, sem bloquear o loop nem outras chamadas à API, e mostra na aba o estado atual: em andamento, sucesso (comandos, linhas devolvidas e afetadas do último comando) ou erro (SQLSTATE e mensagem). Fechar a aba descarta o acompanhamento local; a execução no servidor continua até terminar. Confirmação de `EXPLAIN ANALYZE` não faz parte de SQL-004.

## SQL-005 — Resultados e duração

O servidor guarda o resultado do último comando do script: nomes das colunas e valores das linhas em formato texto do PostgreSQL (`NULL` como `null`). Como o simple query protocol só descreve colunas junto com linhas, um último comando sem linhas tem resultado com `columns` vazio. Comandos anteriores são descartados ao terminar. Cada resultado guarda no máximo 1000 linhas e 5 MiB de valores (soma dos bytes UTF-8); ao atingir um dos limites, as linhas seguintes são contadas, mas não guardadas, e o resultado fica `truncated: true`. Para conter memória, só as 10 execuções concluídas mais recentes de cada usuário mantêm linhas; nas anteriores restam status e contagens.

O status (`GET /api/v1/executions/{id}`) passa a incluir `duration_ms` quando a execução termina (sucesso ou falha), medido no servidor do início da execução, incluindo a conexão, até o fim. Se a tarefa de execução terminar sem registrar resultado (pânico ou abort), a execução fica `failed` com mensagem genérica, sem ocupar vaga de execução indefinidamente.

`GET /api/v1/executions/{id}/result` responde `200` com `{ "columns": ["..."], "rows": [["valor", null]], "stored_rows", "total_rows", "truncated" }` para execução `succeeded` com linhas mantidas. Execução `running` ou `failed` responde `409` (`result_unavailable`); execução cujas linhas foram descartadas pela retenção responde `410` (`result_expired`); inexistente ou de outro usuário, `404`.

Na TUI, ao concluir com sucesso, a aba busca o resultado e o mostra num painel abaixo do editor, com título indicando linhas exibidas, total, truncamento e duração; valores nulos aparecem como `NULL`. No modo de comandos da seção SQL, `Tab` alterna o foco entre editor e resultado (Esc volta às conexões); o resultado usa a grade de dados de `specs/data-grid.md` (GRID-001), com navegação, busca, ordenação e paginação locais sobre as linhas guardadas. A linha de estado mostra a duração em sucesso e em falha. Um novo resultado substitui o anterior da aba; durante uma nova execução o resultado anterior é removido para não ser confundido com o atual.

## SQL-006 — Timeout, cancelamento e EXPLAIN ANALYZE

Cada execução tem um tempo limite único para o script inteiro, contado do início da execução (inclui a conexão). O valor vem de `PGRUSTADMIN_EXECUTION_TIMEOUT_SECS` (inteiro de 1 a 3600, padrão 300); valor inválido impede o servidor de iniciar com mensagem clara. O cliente não escolhe o tempo limite.

Ao conectar, o servidor guarda o PID do backend (`pg_backend_pid()`). Quando o tempo limite estoura ou o usuário cancela, o servidor abandona a conexão da execução e pede ao PostgreSQL o cancelamento do comando em andamento com `pg_cancel_backend(pid)` em uma segunda conexão com a mesma credencial. Se esse pedido falhar, o backend pode continuar até terminar o comando atual, mas o resultado é descartado; a falha não altera o status. Comandos já concluídos em autocommit não são desfeitos.

`POST /api/v1/executions/{id}/cancel` pede o cancelamento de uma execução `running` do usuário e responde `204`; o status muda para `cancelled` quando a execução termina. Execução que não está `running` responde `409` (`execution_not_running`); inexistente ou de outro usuário, `404`. O status passa a ter também `cancelled` e `timed_out`, ambos com `duration_ms`, sem `result` nem `error`; `GET .../result` responde `409` (`result_unavailable`) para eles. Execuções nesses estados não contam no limite de execuções em andamento.

Na TUI, `c` no modo de comandos da seção SQL cancela a execução da aba ativa quando ela está em andamento e o servidor já informou seu ID; antes disso, a TUI pede para tentar novamente. A linha de estado mostra `Cancelled after N ms` ou `Timed out after N ms`.

Antes de executar (`e`), a TUI pede confirmação (`y` executa, `n`/Esc cancela) quando o SQL parece conter `EXPLAIN ANALYZE`, que executa de fato a consulta analisada. A verificação é heurística e só serve de aviso: ignora comentários `--` e `/* */`, compara palavras sem diferenciar maiúsculas e avisa quando uma palavra `EXPLAIN` é seguida de `ANALYZE` ou `ANALYSE` antes do próximo `;`. Pode avisar sem necessidade (por exemplo, dentro de strings) e nunca bloqueia nem altera o SQL.
