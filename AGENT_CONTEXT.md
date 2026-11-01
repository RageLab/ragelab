# Agent Context — RageLab Core

- Última atualização: 2026-11-01 (America/Sao_Paulo).
- Workspace: `D:\DEV\_RageLab\ragelab`.
- Board: `board_015c3e1a-c568-4d7d-a214-f0a247f758d6` — RageLab Core — CLI, Asset Pipeline & Studio Foundation.
- Card atual: `task_00711e72-aeba-4c1c-ae19-623663d54297` — Batch — Pipeline por manifest versionado.
- Branch: `main`; último checkpoint local: `b8d49546d4e41440bf16845609e1feb3dffd6551` (`b8d4954`, Quality/hardening). `origin/main` continua na base inicial `366ba6925e8a402e08759c268bc09a47099418ed`; não foi feito push. Usuário autorizou commits locais incrementais por marco validado; não fazer push sem pedido explícito.
- Preservação: seis exemplos locais de profiling permanecem untracked e intocados. Alterações locais GLB da etapa anterior permanecem na working tree. Não modificar arquivos de instalação GTA/FiveM nem assets externos.
- Nenhuma sessão/terminal do RageLab foi iniciada ou deixada em execução.

## Estado do roadmap

- GLB Validate: implementação concluída e card fechado anteriormente.
- YDR Inspect / Validate: implementação, schemas, CLI, documentação e gates concluídos; card fechado.
- Export — relatório versionado GLB v1: implementação, schema, CLI sidecar, golden fixture, documentação e gates concluídos; card fechado.
- Quality — Harness de fixtures, fuzzing, round-trip e gates independentes: concluído e card fechado.
- Batch — Pipeline por manifest versionado: implementação e testes focados em andamento; commits locais serão feitos por checkpoints validados, sem push.

## Entrega YDR

### Parser
- `crates/ragelab-ydr/src/lib.rs`: novos DTOs `YdrGeometryInspection` e `YdrLodInspection`, função `inspect_drawable_lods`.
- Percorre os slots High, Medium, Low e VeryLow do Drawable, ignora slots vazios e usa o decoder existente `parse_model`.
- Confere a contagem de geometria, tabelas/ranges de bounds por modelo e por geometria, AABBs finitas/ordenadas e limite de geometria.
- Extrai contagens, topology, shader index, declaration flags/types/stride, atributos presentes, finitude de atributos e winding.

### Engine
- Novo `crates/ragelab-engine/src/ydr.rs`, exportado por `ragelab-engine`.
- API pública: `inspect_ydr_bytes` e `validate_ydr_bytes`.
- Reports versionados `ragelab.ydr.inspection` v1 e `ragelab.ydr.validation` v1, com container RSC7, Drawable/bounds, todos os LODs/geometrias, ShaderFX/params/samplers, TextureDictionary, hashes e diagnósticos.
- Validate verifica parse tipado, estrutura, bounds, triangle-list cardinality, atributos não finitos, shader/texture references, coerência de name hash e dimensões/formato de textura; decodifica todos os mip levels suportados.
- `--strict` promove warnings para errors. Exit code 3 para features/formats reconhecidos mas unsupported; exit code 4 para dado inválido/strict warnings.
- Semantic reopen reserializa RSC7 apenas em memória e reabre/compara segmentos e metadados sem gerar arquivo.
- `runtimeRenderable=false` e `codewalkerGuiAccepted=false` permanecem explícitos.

### CLI / contratos
- Novo `apps/ragelab/src/ydr_cmd.rs`.
- Comandos `ragelab ydr inspect <file.ydr> [--json]` e `ragelab ydr validate <file.ydr> [--strict] [--json]`, com IDs estruturados `ydr.inspect` e `ydr.validate`.
- Falhas de validação mantêm o relatório completo no campo `data` do envelope CLI.
- Capabilities, normalização, structured output e help atualizados.
- Schemas Draft 2020-12:
  - `docs/schemas/ragelab.ydr.inspection.v1.schema.json`
  - `docs/schemas/ragelab.ydr.validation.v1.schema.json`
- README, `docs/CAPABILITIES.md`, `docs/CLI_CONTRACT.md` e `docs/FORMAT_SUPPORT_MATRIX.md` atualizados.
- Novos testes: `apps/ragelab/tests/ydr_cli.rs`; regressões de LOD em `ragelab-ydr`; testes diretos do Engine em `ragelab-engine/src/ydr.rs`.

## Gates executados

- `cargo test --locked --workspace`: PASS. A suíte total passou; um teste de YFT real é ignorado por exigir `RAGELAB_TEST_YFT` apontando para asset GTA V Legacy externo.
- `cargo test --locked -p ragelab-ydr`: 35/35 PASS.
- `cargo test --locked -p ragelab-engine --lib ydr::tests`: 4/4 PASS; a suíte de Engine completa passou no workspace.
- `cargo test --locked -p ragelab --test ydr_cli`: 5/5 PASS.
- `cargo test --locked -p ragelab --test cli_contract`: 33/33 PASS.
- `cargo clippy --locked -p ragelab-engine --lib --tests -- -D warnings`: PASS.
- `cargo clippy --locked -p ragelab --all-targets --no-deps -- -D warnings`: PASS.
- `cargo fmt --all -- --check`: PASS.
- Schemas Draft 2020-12 validados contra relatórios reais de `simple.ydr` e `embedded.ydr`: PASS.
- `git diff --check`: PASS.

### Nota sobre Clippy e profiling local
Uma execução de `clippy --all-targets` no Engine incluiu os seis exemplos de profiling locais untracked; três deles acionaram `field_reassign_with_default`. Esses exemplos são preexistentes e foram preservados. O gate estrito direcionado ao Engine lib/tests e o gate da CLI passaram. Não editar esses exemplos sem pedido explícito.

## Limites conhecidos

- Semantic reopen comprova consistência do round-trip do parser/serializer RageLab, não aceitação por CodeWalker GUI, GTA V ou FiveM.
- Nenhum runtime/GUI externo foi executado nesta etapa.
- A inspeção compreende os quatro slots LOD documentados; o modelo histórico `YdrDocument::model` continua expondo o primeiro LOD não vazio para compatibilidade, enquanto `inspect_drawable_lods` e o novo report expõem todos os LODs.
- Não alegar `runtimeRenderable=true` nem aceitação externa com base nestes gates.

## Entrega Export — GLB export report v1

### CLI / contrato
- Novo módulo `apps/ragelab/src/glb_export_report.rs`, conectado por `apps/ragelab/src/main.rs`.
- `ragelab glb export <source.glb> --output <output.ydr> --report <report.json>` grava um sidecar separado. Sem `--report`, o fluxo e o envelope JSON anterior permanecem inalterados.
- O caminho do sidecar é exposto como `data.report` em JSON e como linha `report: ...` em saída humana.
- O sidecar `ragelab.glb.export-report` v1 é gerado a partir dos bytes de entrada, perfil efetivo, template e inspeção tipada do YDR final. Contém hashes SHA-256/byte lengths, tool version + Core revision/dirty-state, origem/configuração do perfil, template, decisões de material/geometria, slots e samplers, parâmetros ShaderFX e referências realmente serializadas.
- Detalha transformações de hierarquia/coordenadas/topologia/bounds, PNG→RGBA8, formato de textura e mipmaps, inclusões/omissões, compressão com perda autorizada pelo perfil, fallback do shader neutro para geometria sem material, warnings/limitations e status de cada fase.
- A saída sempre mantém `runtimeRenderable=false`; CodeWalker Core/GUI, GTA V e FiveM ficam `not_run`. O sidecar é produzido somente depois de exportação bem-sucedida; entradas/formatos/features não suportados continuam falhando em modo fail-closed pelo envelope de erro da CLI, sem produzir um relatório de sucesso.
- Não há timestamp nem nomes de arquivos temporários no JSON. Para mesmos bytes, opções e paths de input/output, o sidecar é byte-idêntico.
- `--report` deve ser path novo e distinto de source/profile/template/output. Publicação usa arquivo temporário no mesmo diretório e hard-link atômico no destino para nunca substituir um sidecar existente. `--overwrite` controla apenas o YDR.
- Comparação de aliases de caminhos é case-insensitive no Windows.

### Schema, fixture e documentação
- Schema Draft 2020-12: `docs/schemas/ragelab.glb.export-report.v1.schema.json`.
- Golden snapshot sintético: `fixtures/synthetic/glb-export-report-v1.golden.json`; política descrita em `fixtures/synthetic/README.md`.
- Documentação em `docs/GLB_IMPORT.md`, `docs/CLI_CONTRACT.md` e referência do README.
- Testes em `apps/ragelab/tests/glb_cli.rs`: sidecar v1, SHA, decisões, golden snapshot, repeat determinism, equivalência humano/JSON, alias/no-overwrite; testes condicionais cobrem template YFT e fallback ShaderFX se a variável `RAGELAB_SHADER_TEMPLATE_YFT` fornecer fixture externa read-only. No workspace desta execução, essa variável não estava configurada, então esses dois testes condicionais retornaram sem usar asset proprietário.

### Gates executados em 2026-11-01
- `cargo test --locked --workspace`: PASS; um teste preexistente de YFT real permaneceu ignored por exigir `RAGELAB_TEST_YFT`.
- `cargo test --locked -p ragelab --test glb_cli`: 34/34 PASS.
- `cargo test --locked -p ragelab --test cli_contract`: 33/33 PASS.
- `cargo clippy --locked -p ragelab --all-targets --no-deps -- -D warnings`: PASS.
- `cargo clippy --locked -p ragelab-engine --lib --tests -- -D warnings`: PASS.
- `cargo fmt --all -- --check`: PASS.
- `git diff --check`: PASS.
- Um sidecar real produzido pela CLI com GLB sintético foi validado com `jsonschema.Draft202012Validator` contra o schema publicado: PASS.
- Teste de golden snapshot, repetição byte a byte e equivalência entre modo humano e `--json`: PASS.

### Git / preservação
- Nenhum staging, commit ou push foi executado.
- `main` e `origin/main` seguem em `366ba6925e8a402e08759c268bc09a47099418ed`; as entregas GLB Validate, YDR e Export permanecem locais.
- Os seis exemplos locais de profiling untracked continuam preservados e não foram editados.
- Nenhum arquivo da instalação GTA/FiveM ou asset proprietário foi modificado.
- `RAGELAB_SHADER_TEMPLATE_YFT` estava ausente; logo a validação de sidecar com shader real fica pendente de fixture autorizada, embora o teste correspondente esteja implementado e condicionado à variável.

## Entrega Quality — fixture harness, mutation tests e acceptance gates

### Política de fixtures e harness
- Novo documento `docs/QUALITY.md`: política para sintéticos no Git, assets proprietários externos/read-only, instalação probes metadata-only, gates por camada, budgets, mutation smoke tests e comandos locais/CI.
- `scripts/check_hardening.py` agora valida:
  - todos os oito gate IDs e status separados (repository, semantic reopen, CodeWalker Core, GUI, GTA Legacy, FiveM);
  - evidência histórica CodeWalker Core rev `485d56b` (5 aceitos/0 rejeitados);
  - evidência GUI user-confirmed da prévia v3 (Drawn 140 geom);
  - FiveM `PASS_USER_CONFIRMED` para os cinco presets e `runtimeRenderableScope=FiveM_only`;
  - GTA V Legacy standalone em `waived` com `requiresExplicitApprovalToReopen=true`;
  - budgets explícitos de GLB, geometry, imagens, RBF, YDR, YBN, YTD e manifests Steam/CitizenFX;
  - presença dos gates `--locked` e validação de fixtures no workflow CI.
- `fixtures/hardening/compatibility-matrix.json` mantém as oito fixtures sintéticas distribuídas em base/update/dlc/world/interior e dois perfis Legacy sintéticos. Inclui os gates e as evidências externas históricas sem promovê-las para CI.

### Robustez e limites
- `crates/ragelab-engine/src/discovery.rs`: helper `read_bounded_text` checa metadata e lê no máximo limite + 1 byte; `libraryfolders.vdf` e `appmanifest_*.acf` limitados a 1 MiB cada; `CitizenFX.ini` limitado a 256 KiB. Leitura oversized/invalid UTF-8 falha fechada e o discovery ignora o metadata inválido.
- Testes de limite oversized e 256 mutações/truncamentos para parse de manifest VDF/ACF.
- `crates/ragelab-engine/src/glb.rs`: 160 mutações de GLB válido + 160 entradas aleatórias até 256 bytes, sem panic; quando o decode tem êxito, valida hash/byte length comuns e cardinalidade vertex/index por primitive.
- `crates/ragelab-rbf/src/lib.rs`: 256 inputs mutados/aleatórios até 512 bytes, incluindo prefixo RBF0 e truncamentos, sem panic.
- `crates/ragelab-resource/src/lib.rs`: 256 variações de header RSC7 e 128 mutações/truncamentos de resource com expectativa de descompressão fixa em 512 bytes, sem panic.
- São deterministic mutation smoke tests, não libFuzzer/AFL nem prova de cobertura/exaustividade. Nenhuma crate nova de fuzzing foi introduzida.
- Budgets atuais documentados em `docs/QUALITY.md`: GLB 512 MiB; 2M vertices/6M indices por primitive; imagem individual 128 MiB/agregado 512 MiB/64M pixels/16,384 dimensão; RBF depth 256/nodes 1M; YDR 4,096 models/shaders, 16,384 geometrias por model, 2M/6M vertex/index e 256 MiB vertex bytes; YBN 16,384 children/2M vertices/4M polygons/4M vertices e 24M indices flattened; YTD 16,384 textures/2 GiB payload/256 MiB decoded RGBA/512 MiB generated mips; metadata Steam 1 MiB e CitizenFX 256 KiB.
- Round-trip/golden existentes permanecem obrigatórios: writers reabrem a saída e comparam estruturas/segmentos/payloads; export report v1 tem golden projection sintético e teste byte-for-byte reproducible.

### Gates e evidência independente
- `.github/workflows/ci.yml`: Rust 1.80 + rustfmt/clippy; scripts de fixture/hardening; `cargo fmt --all -- --check`; `cargo check --locked --workspace`; `cargo clippy --locked --workspace --all-targets -- -D warnings`; `cargo test --locked --workspace`; build visual também usa `--locked`. Visual regression roda contra sintéticos com tolerância zero; matriz web permanece Chromium/Firefox/WebKit.
- CodeWalker Core: evidência anterior 5/5 no revision `485d56b` (Card06); resultado manual externo, sem adapter CI redistribuível.
- CodeWalker GUI: usuário confirmou que a prévia `stylized_halloween_asset_preview_v3.ydr` estava visível, `Drawn: 140 geom`, 28 geometries Default/PNCTT (Card08). Evidência é daquele YDR/GUI, não generalizada para novos outputs.
- FiveM: usuário confirmou PASS para `normal.sps`, `spec.sps`, `normal_spec_tnt.sps`, `emissivenight.sps` e `emissive.sps`; logs/screenshots não foram arquivados. `runtimeRenderable=true` vale somente para FiveM e esse pacote/presets.
- GTA V Legacy standalone: WAIVED/NOT RUN por decisão explícita do usuário. Nenhum deploy/runtime foi preparado ou executado nesta tarefa.

### Gates executados em 2026-11-01
- `python scripts/validate_synthetic.py`: PASS.
- `python scripts/validate_synthetic_ymf.py`: PASS.
- `python scripts/check_hardening.py`: PASS; valida 8 cases/5 layers/2 synthetic build profiles e as regras dos gates.
- `cargo check --locked --workspace`: PASS.
- `cargo test --locked --workspace`: PASS; um teste YFT real permanece ignored por exigir `RAGELAB_TEST_YFT`.
- Mutation tests direcionados: discovery 11/11; GLB 1/1; RBF 1/1; RSC7 1/1.
- `cargo clippy --locked --workspace --lib --tests -- -D warnings`: PASS.
- `cargo clippy --locked -p ragelab --all-targets --no-deps -- -D warnings`: PASS.
- `cargo clippy --locked -p ragelab-engine --lib --tests -- -D warnings`: PASS.
- `cargo clippy --locked -p ragelab-rbf --all-targets -- -D warnings`: PASS.
- `cargo clippy --locked -p ragelab-resource --all-targets -- -D warnings`: PASS.
- `cargo fmt --all -- --check`: PASS; `git diff --check`: PASS (Git emitiu apenas warning CRLF→LF para o arquivo RBF já em CRLF).
- A tentativa do comando exato CI `cargo clippy --locked --workspace --all-targets -- -D warnings` falhou somente em três dos seis exemplos de profiling locais untracked (`world_prefetch_profile.rs`, `world_asset_overlap.rs`, `world_ydd_cache_profile.rs`) por `field_reassign_with_default`. Os seis exemplos foram preservados e não editados. O CI em checkout limpo não contém esses untracked; o gate workspace lib/tests e os alvos app/RBF/Resource passaram.

### Git / preservação
- Checkpoint local criado: b8d49546d4e41440bf16845609e1feb3dffd6551 — test(quality): add fixture and hardening gates (8 arquivos, 492 inserções/7 remoções).
- main está um commit à frente de origin/main; nenhum push foi executado.
- Seis exemplos de profiling untracked preservados e não editados.
- Nenhum asset proprietário ou arquivo da instalação GTA/FiveM foi modificado.

## Entrega Batch — progresso da implementação

### Implementação atual

- Novo apps/ragelab/src/glb_batch.rs: DTO de manifest estrito v1 (deny_unknown_fields), com shaderTemplate requerido e nullable, shaderIndex, nodes, profile e report explícitos por item.
- Manifest limitado a 1..128 itens; concorrência limitada a 1..8; budgets de manifest 4 MiB, GLB 512 MiB, profile 16 MiB e template 256 MiB.
- Paths relativos resolvidos contra o diretório do manifest; rejeita absoluto/drive/root/dot/dotdot, symlink escape, destino existente e colisões entre aggregate/YDR/item report/input/manifest; diretórios de saída devem preexistir.
- Preflight global roda o export byte-first verdadeiro do Engine, profile validation, template validation, seleção de nodes, semantic YDR reopen e construção do report individual para todos os itens antes de qualquer publicação. Hashes SHA-256 são conferidos novamente na execução para detectar alteração de inputs após o preflight.
- Worker pool usa std::thread::scope limitado pela concorrência, retorna itens na ordem original do manifest e publica YDR + sidecar individual atomicamente/no-replace. Relatório agregado ragelab.glb.batch-report v1 é publicado ao final.
- continue isola falhas por item; fail_fast impede novos agendamentos após primeira falha, aguarda os workers já ativos e marca os não iniciados como cancelled. Atomicidade é por arquivo, não transação de lote; outputs já concluídos não são revertidos quando uma execução posterior falha.
- Dispatch/normalização/capabilities/structured output/help CLI conectados para ragelab glb batch <manifest.json> [--json]; erro batch_failed mantém agregado no envelope JSON.
- Schemas Draft 2020-12: docs/schemas/ragelab.glb.batch-manifest.v1.schema.json e docs/schemas/ragelab.glb.batch-report.v1.schema.json. Documentação em docs/GLB_BATCH.md, README e CLI contract.
- Testes: 7 unit tests cobrindo strict schema, required nullable template, path escape, preflight global sem writes, colisão de destinos, continue isolamento, fail-fast cancellation, ordenação e determinismo byte a byte; 1 end-to-end CLI com export real + sidecar + aggregate.
- Gates completos executados: cargo check --locked --workspace PASS; cargo test --locked --workspace PASS (um teste YFT externo ignored por exigir RAGELAB_TEST_YFT); cargo clippy --locked --workspace --lib --tests -- -D warnings PASS; cargo fmt --all -- --check PASS; git diff --check PASS; python scripts/validate_synthetic.py, validate_synthetic_ymf.py e check_hardening.py PASS; schemas Draft 2020-12 + exemplos válidos/inválidos PASS. Batch unit 7/7; GLB CLI 35/35; CLI contract 33/33; Clippy app --all-targets --no-deps PASS. O comando CI exato cargo clippy --locked --workspace --all-targets -- -D warnings falha somente nos três exemplos locais world_lod_profile.rs, world_prefetch_profile.rs e world_asset_overlap.rs por field_reassign_with_default; seis exemplos de profiling foram preservados sem edição. Pendentes: revisão do staging, commit do grupo asset pipeline e fechamento do checklist/auditoria.

## Próxima ação

- Próximo card após Batch: Diagnostics — ragelab doctor e capabilities (task_b7672e16-46ab-4c6d-8bc5-5bbc52a37d39).
- Não reabrir as etapas GLB/YDR sem regressão ou pedido explícito; não reabrir o gate standalone GTA V Legacy previamente waived sem autorização.
