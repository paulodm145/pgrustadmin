# Grade de dados

## GRID-001 — Grade reutilizável, prévia de tabelas e resultados

Um único componente de grade da TUI exibe tanto o resultado de consultas (SQL-005) quanto a prévia de dados de uma relação do catálogo. Por padrão a grade desenha linhas no estilo planilha: o cabeçalho e cada registro são sublinhados de ponta a ponta da grade, na cor de texto secundário, e uma linha vertical separa as colunas; o sublinhado não ocupa linhas de tela, então cada registro continua em uma linha. Em terminais sem cor de sublinhado, ele usa a cor do texto. `L` liga e desliga as linhas durante a sessão. A grade mostra uma coluna fixa com o número da linha, cabeçalho com indicador de ordenação (`▲`/`▼`), valores nulos como `NULL` atenuado e valores longos truncados com `…`. A linha sob o cursor e a célula ativa ficam destacadas.

Navegação com a grade em foco: cima/baixo movem a linha ativa dentro da página; esquerda/direita mudam a coluna ativa, rolando as colunas quando necessário; Home/End vão à primeira/última linha da página; `[`/`]` e PgUp/PgDn trocam de página; `g`/`G` vão à primeira/última página; `+`/`-` alternam o tamanho de página entre Auto, 10, 25, 50 e 100; `s` alterna a ordenação pela coluna ativa (crescente, decrescente, sem ordem); `/` abre o campo de busca (Enter confirma, Esc limpa); Enter abre um popup com o valor completo da célula ativa (Esc fecha). Auto usa as linhas que cabem na área visível; páginas maiores que a área rolam dentro da página. Mudar busca, ordenação ou tamanho de página volta à primeira página.

O rodapé da grade mostra o intervalo exibido e o total (`linhas 26–50 de 1000`), busca e ordenação ativas, o tamanho de página e os números de página com `«` `‹` e `›` `»` indicando se há páginas anteriores/seguintes.

Busca é um filtro: mantém apenas linhas em que algum valor contém o texto, sem diferenciar maiúsculas/minúsculas.

### Resultados de consulta (fonte local)

Como o SQL do usuário não é analisado nem reescrito, busca, ordenação e paginação atuam sobre as linhas guardadas pelo servidor (até 1000, SQL-005); um resultado truncado é indicado no rodapé. A ordenação é por tipo inferido: se todos os valores não nulos da coluna são números, ordena numericamente; se todos são datas/horários ISO (`AAAA-MM-DD...`), ordena cronologicamente; caso contrário, por texto. Nulos ficam por último em ambas as direções.

### Prévia de dados de relações (fonte remota)

`GET /api/v1/connections/{id}/databases/{database}/schemas/{schema}/relations/{relation}/rows` aceita `page`, `per_page` (como conexões: padrão 25, máximo 100), `sort` (nome de uma coluna da relação), `direction` (`asc`, padrão, ou `desc`) e `search`. Responde `200` com `{ "columns": ["..."], "rows": [["valor", null]], "total_rows", "page", "per_page" }`, com valores no formato texto do PostgreSQL e colunas na ordem da relação. A ordenação usa o tipo real da coluna, com nulos por último; sem `sort`, a ordem é a do PostgreSQL (não garantida). A busca compara o texto da linha inteira (`registro::text ILIKE`) com `%`, `_` e `\` do termo escapados; `total_rows` conta as linhas que atendem à busca.

Identificadores vêm do catálogo e são sempre citados (`"..."` com aspas duplicadas escapadas); valores de busca e paginação são parâmetros. Relação inexistente, de tipo fora de CAT-003 ou em schema sem `USAGE` responde `404`; a TUI informa que a tabela não foi encontrada ou que o servidor está desatualizado (um servidor anterior a esta rota também responde `404`); `sort` que não é coluna da relação, paginação inválida, `direction` inválida ou busca com mais de 200 caracteres respondem `400`; falta de privilégio de leitura responde `403` (`relation_not_readable`). Autorização, validação de identificadores, `503` de chave e `502` sem mensagem do driver seguem CAT-002. A operação tem limite de 15 segundos (conexão e consultas).

Na TUI, o detalhe da relação mostra colunas e índices lado a lado e a grade de dados abaixo, carregada ao abrir; `Tab` alterna o foco entre a lista de colunas e a grade, e Esc volta à lista de relações. No resultado de consultas, `Tab` continua alternando entre editor e grade.
