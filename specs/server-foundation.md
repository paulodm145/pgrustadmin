# Especificação: fundação do servidor

## Objetivo

Preparar a inicialização do servidor para que tarefas posteriores possam acrescentar rotas sem misturar configuração, observabilidade e comportamento da API.

## Configuração inicial

- `PGRUSTADMIN_BIND_ADDR` define o endereço de escuta como `IP:PORT`.
- Na ausência da variável, usar `127.0.0.1:3000`.
- Endereço inválido ou valor não Unicode encerra o processo com erro claro, sem revelar o valor configurado.
- Esta etapa aceita somente endereços de loopback. Bind remoto continuará recusado até que TLS e autenticação estejam implementados e configurados.

## Logs e falhas de inicialização

- Usar `tracing` e escrever logs no stderr.
- `RUST_LOG` define o filtro; sem a variável, usar `info`.
- Filtro inválido, falha de bind ou falha ao iniciar o servidor encerra o processo com código diferente de zero e mensagem sem segredos ou stack trace.
- Ao iniciar, registrar o endereço efetivamente aberto.

## Limites desta etapa

O servidor ainda não registra rotas. Contrato JSON de erros HTTP, health/version e demais endpoints pertencem à tarefa API-001, conforme `specs/api.md`. Não adicionar configuração de banco, autenticação ou segredos nesta etapa.

## Critérios de aceite

1. Servidor inicia em loopback por padrão e aceita endereço explícito via `PGRUSTADMIN_BIND_ADDR`.
2. Configuração inválida falha sem expor o valor recebido.
3. Logs usam `RUST_LOG` ou filtro padrão `info` e mostram o endereço ativo.
4. Falhas de inicialização resultam em saída não zero e mensagem segura.
5. `cargo fmt --all -- --check` e `cargo check --workspace` passam.
