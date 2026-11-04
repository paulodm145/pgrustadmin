# ADR-0003 — Proteção de credenciais
- Status: Aceito com detalhes de implementação pendentes
- Data: 2026-11-02

Senhas de login: hash Argon2id, nunca criptografia reversível. Credenciais dos bancos externos: criptografia autenticada AEAD em repouso. Chave mestra fora do banco, fornecida por secret manager, arquivo protegido ou mecanismo do SO.

Para o início do desenvolvimento, a chave mestra será fornecida por variável de ambiente. A variável não deve ser registrada em logs nem persistida no banco. Esta escolha inicial foi confirmada em 2026-11-04.

Antes de implementar CON-002, especificar formato versionado do envelope, backup/recuperação, rotação e comportamento sem a variável de ambiente. Avaliar a operação em produção antes de ampliar a decisão de fonte da chave. Usar biblioteca mantida e algoritmo estabelecido; não criar criptografia própria. Nunca registrar segredos.
