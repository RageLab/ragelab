# CLI, Core Rust e Studio — Arquitetura e contratos

**Status:** decisão arquitetural para a próxima fase do RageLab Core
**Data:** 2026-10-31
**Escopo:** CLI, serviços compartilhados Rust, contratos serializados e integração Tauri do RageLab Studio.

Este documento complementa [ARCHITECTURE.md](ARCHITECTURE.md), [CLI_CONTRACT.md](CLI_CONTRACT.md), [CAPABILITIES.md](CAPABILITIES.md) e [GLB_IMPORT.md](GLB_IMPORT.md). Não substitui os contratos atuais; registra as fronteiras de aplicação compartilhada e o estado efetivamente implementado de cada adapter.

## 1. Decisões

1. **`ragelab-engine` continua sendo a camada de aplicação compartilhada.** Não criar uma segunda fachada genérica chamada `ragelab-core` ou equivalente. A arquitetura existente já estabelece o engine como a superfície transport-neutral para CLI, Studio e futuros adapters.
2. **Crates de formato são autoridades sobre seus formatos.** Parser, writer, invariantes binárias, referências, layout, compressão e limites pertencem às crates especializadas (`ragelab-gltf`, `ragelab-ydr`, `ragelab-ytd`, `ragelab-ydd`, `ragelab-ybn`, `ragelab-yft`, `ragelab-ymap`, `ragelab-ytyp`, `ragelab-ymf`, `ragelab-rpf` e camadas binárias).
3. **`ragelab-authoring` mantém operações de authoring de baixo nível e byte-oriented** que podem ser reutilizadas pelo engine. O engine coordena fluxos entre formatos, políticas de operação, elegibilidade e validação pós-escrita.
4. **CLI e Tauri são adapters, não implementações de domínio.** Podem tratar argumentos, seleção de arquivos, filesystem, UI, renderização, eventos e serialização de transporte; não podem duplicar regra de material, shader, formato, validação ou segurança do writer.
5. **Studio chama os serviços Rust diretamente.** Não deve iniciar `ragelab.exe`, interpretar stdout, reproduzir os argumentos da CLI ou manter uma segunda implementação de conversão. CLI continua sendo a interface pública para terminal, automação e scripts.
6. **Contratos públicos são versionados por domínio.** Envelope CLI, DTOs do engine, profiles, manifests, relatórios e pacotes de renderização têm schemas/versões independentes. A versão do executável não substitui a versão do contrato.
7. **Comportamento fail-closed e evidência explícita são obrigatórios.** Sem fallback, descarte de atributos, conversão de material, escolha de shader ou aceitação runtime implícitos.

## 2. Arquitetura observada

### 2.1 O que já está corretamente separado

- `ragelab-resource`, `ragelab-meta`, `ragelab-pso`, `ragelab-rbf` e as crates tipadas de formato contêm parsing/serialização especializada.
- `ragelab-assets` concentra indexação, resolução de providers e operações de workspace.
- `ragelab-authoring` oferece APIs byte-oriented reutilizáveis para relatórios de textura/material, exportação e substituição de conteúdo YTD e análise de bindings YDR/YDD.
- `ragelab-engine` já é a camada de aplicação transport-neutral. Seus módulos cobrem inspeção/capabilities, operações declarativas, discovery, preview, cena, RPF, spatial, authoring YMAP, streaming e workspace export. O módulo `texture_authoring` já reexporta operações de `ragelab-authoring`.
- O backend Tauri do Studio já consome `ragelab-engine` para inspeção, capabilities, preview, workspace operations/export e authoring de YMAP; `ragelab-render` é usado como serviço de renderização nativa.
- Os testes existentes `apps/ragelab/tests/cli_contract.rs` e `glb_cli.rs` verificam envelopes JSON, códigos de saída, capabilities, comandos namespaced e relatórios GLB.

### 2.2 GLB→YDR — use case compartilhado implementado

A orquestração GLB→YDR foi extraída de `apps/ragelab/src/glb_cmd.rs` para `crates/ragelab-engine/src/glb.rs`. O módulo do engine coordena as crates especializadas e expõe funções byte-first para inspect, decode de geometria, criação/validação de profile e export:

- `inspect_glb_bytes` e `decode_glb_geometry_bytes`;
- `default_glb_authoring_profile` e `validate_glb_authoring_profile_bytes`;
- `export_glb_to_ydr(GlbYdrExportRequest)`.

O request de export contém `source_name` + `source_bytes`, bytes opcionais de profile e `GlbShaderTemplateInput` explícito (formato YFT Legacy ou YDR Legacy, nome de provenance e bytes). O contrato do relatório é versionado em `docs/schemas/ragelab.glb.ydr-export.v1.schema.json`. O engine não abre paths, não consulta environment variables, não procura templates no disco e não escolhe um shader por heurística. `ragelab-gltf` continua autoridade do parser/decoder/profile; `ragelab-yft` lê o template YFT suportado; `ragelab-ydr` constrói e reabre semanticamente o resultado; `ragelab-ytd` fornece formatos de textura.

O use case mantém as cinco assinaturas ShaderFX estritas já habilitadas, valida UV sets/atributos, faz o decode de imagens e monta o YDR. Retorna `output_bytes` e `GlbYdrExportReport`, com SHA-256 de source/profile/template/output, contagens, bindings, formatos de textura e `semanticReopen`. A reabertura pelo parser YDR é obrigatória antes de devolver os bytes. `runtimeRenderable` permanece `false`: parser reopen não equivale a aceitação no GTA V ou no CodeWalker.

A CLI agora é adapter de argumentos, paths, leitura/escrita e apresentação. Ela chama os use cases do engine e mantém o envelope CLI v1 e o comportamento de saída sem overwrite implícito. O Studio tem comandos Tauri `core_glb_inspect`, `core_glb_geometry`, `core_glb_profile_init`, `core_glb_profile_validate` e `core_glb_export`; esses comandos chamam as mesmas funções Rust diretamente, sem subprocesso CLI. Os wrappers TS ficam em `src/lib/native.ts` e refletem os DTOs serializados do Rust.

O módulo dispatcher geral `apps/ragelab/src/main.rs` continua com comandos históricos e aliases flat. Essa migração não tenta reescrever a CLI inteira.

### 2.3 Metadata de build e compatibilidade de API

A constante `RAGELAB_CORE_REVISION` fixa foi removida do Studio. `crates/ragelab-engine/build.rs` injeta `CORE_REVISION` com o HEAD Git do checkout, aceita `RAGELAB_CORE_REVISION` como override explícito para builds reproduzíveis e usa `unknown` se o Git não estiver disponível. O build também determina `CORE_DIRTY` via Git e expõe `CORE_API_COMPATIBILITY_VERSION` separadamente.

O `StudioInfo` expõe agora `coreRevision`, `coreDirty` e `coreApiCompatibility`. Revision identifica o commit de origem; dirty informa que o checkout tinha alterações no momento da compilação; o número de compatibilidade é o contrato explícito de API para adapters. Nenhum desses valores deve ser inferido a partir da versão do executável Studio.

## 3. Camadas e responsabilidades

```text
                  RageLab Studio (Svelte/TypeScript)
                               |
                     Tauri command adapter
                               |
                               v
 CLI (args/stdout/exit) ---> ragelab-engine <--- future MCP/API adapter
                               |
               +---------------+----------------+
               |               |                |
        ragelab-authoring  ragelab-assets   scene/workspace services
               |                                |
               +---------------+----------------+
                               |
                   format-specific crates
                               |
        resource / RSC7 / META / PSO / RBF primitives

         ragelab-render: rendering service consuming engine
         RenderPackage; presentation/viewport concern, not
         a second parser or asset-authoring implementation.
```

| Camada | Responsabilidade | Não deve fazer |
|---|---|---|
| Binary primitives | Bounds checks, readers/writers binários, compressão/container | Conhecer UI, CLI, Tauri ou paths do usuário |
| Format crates | Modelos tipados, parsing, validação específica, serialização e invariantes do formato | Escolher política do produto ou formatar mensagens de terminal |
| `ragelab-authoring` | Helpers byte-oriented de authoring e semântica específica de bindings/texturas | Depender de CLI/Tauri ou escrever arbitrariamente no filesystem |
| `ragelab-assets` | Indexação, provider lookup, dependency closure e resource assembly | Expor protocolo de transporte ou UI |
| `ragelab-engine` | Use cases compartilhados, coordenação entre crates, política de elegibilidade, capabilities, diagnósticos, non-destructive output e semantic verification | Depender de CLI, Tauri, Svelte, HTTP ou process-global UI state |
| CLI | Argument parsing, aliases, filesystem adapter, human output, JSON envelope, exit code e progresso em stderr | Reimplementar material mapping, shader policy, parser/writer ou eligibility |
| Tauri adapter | Dialog/path selection, desktop/cache integration, command DTOs, events, native-window lifecycle | Duplicar operações do engine ou iniciar o CLI como subprocesso |
| Svelte/TypeScript | Estado visual, editor, UX, apresentação de reports e eventos | Interpretar binários ou decidir suporte de formato |

### Política de I/O

Operações de asset isolado devem preferir APIs baseadas em bytes e dados tipados. O adapter lê o arquivo, entrega os bytes ao use case e grava o resultado através da política compartilhada de saída. Isso permite que CLI e Tauri usem a mesma conversão sem obrigar o engine a conhecer um terminal.

Operações de workspace/RPF naturalmente precisam de contexto de origem. Devem continuar usando as abstrações e requests explícitos existentes do engine (workspace root, fallback roots, RPF mounts, game index e keys). Não introduzir um sistema genérico de plugins/providers para operações simples de arquivo.

Toda escrita que o produto declarar non-destructive deve:
- exigir destino explícito e distinto da fonte;
- não sobrescrever por padrão;
- fazer preflight antes de criar o destino;
- escrever atomicamente quando a operação e o filesystem permitirem;
- reabrir/validar semanticamente o output antes de reportar sucesso;
- preservar a fonte e reportar qualquer falha de verificação.

## 4. Contrato de serviços compartilhados

### 4.1 Operações GLB como serviço do engine

O use case está implementado em `crates/ragelab-engine/src/glb.rs`. A API pública é byte-first, síncrona e não conhece paths, filesystem, ambiente ou Tauri:

```rust
pub fn inspect_glb_bytes(source_bytes: &[u8], source_name: &str)
    -> Result<GlbInspectionReport, io::Error>;

pub fn decode_glb_geometry_bytes(source_bytes: &[u8], source_name: &str)
    -> Result<DecodedGeometryReport, io::Error>;

pub fn default_glb_authoring_profile(source_bytes: &[u8], source_name: &str)
    -> Result<GtaAuthoringProfile, io::Error>;

pub fn validate_glb_authoring_profile_bytes(
    source_bytes: &[u8], source_name: &str, profile_bytes: &[u8],
) -> Result<ProfileValidationReport, io::Error>;

pub struct GlbShaderTemplateInput {
    pub format: GlbShaderTemplateFormat,
    pub source_name: String,
    pub bytes: Vec<u8>,
}

pub struct GlbYdrExportRequest {
    pub source_name: String,
    pub source_bytes: Vec<u8>,
    pub profile_bytes: Option<Vec<u8>>,
    pub shader_template: Option<GlbShaderTemplateInput>,
    pub shader_index: usize,
}

pub struct GlbYdrExportResult {
    pub output_bytes: Vec<u8>,
    pub report: GlbYdrExportReport,
}

pub fn export_glb_to_ydr(request: GlbYdrExportRequest)
    -> Result<GlbYdrExportResult, io::Error>;
```

O Engine coordena profile validation, seleção estrita do template ShaderFX, decode/conversão das imagens, bindings material/geometry/sampler, writer YDR e semantic reopen. `ragelab-gltf`, `ragelab-yft`, `ragelab-ytd` e `ragelab-ydr` permanecem authorities de parsing/formatos. O relatório inclui hashes/provenance e mantém `runtimeRenderable=false`.

CLI e Studio chamam exatamente esses use cases. A CLI adapta args/filesystem/JSON envelope/exit codes. Tauri adapta paths/dialogs, lê os bytes e grava o output com `create_new` por padrão; overwrite requer opt-in explícito. Nenhum adapter inicia o binário CLI como subprocesso.

Os DTOs GLB usados no frontend são gerados a partir das structs Rust com `ts-rs`. Em `ragelab-studio`, execute `bun run generate:glb-types`; os arquivos ficam em `src/lib/generated/glb`. `src/lib/native.ts` importa esses bindings e conserva aliases de nomes públicos antigos. O teste Tauri `export_glb_typescript_bindings` executa a mesma geração de forma reprodutível.

O build do Engine injeta `CORE_REVISION` e `CORE_DIRTY` por metadata de Git, e expõe `CORE_API_COMPATIBILITY_VERSION`. O Studio publica esses mesmos valores em `StudioInfo`, sem manter uma revisão hardcoded.

### 4.2 Reports e diagnósticos

Os novos serviços devem expor resultados serializáveis e estáveis. A forma lógica mínima é:

```text
OperationReport<T>
  operation
  operationVersion
  status
  data: T
  diagnostics[]
  validationPhases[]
  provenance
```

`status` distingue `completed`, `completedWithWarnings`, `rejected`, `failed` e `cancelled`. Erros fatais que impedem a produção de um report retornam um `CoreError` tipado.

Cada diagnóstico deve conter:
- `code`: identificador estável e namespaced, por exemplo `GLB.ACCESSOR_OUT_OF_BOUNDS`;
- `severity`: `info`, `warning` ou `error`;
- `stage`: `container`, `structural`, `semantic`, `compatibility`, `authoring`, `write` ou `externalAcceptance`;
- `message`: descrição humana, não usada para branching;
- `location`: JSON Pointer, byte offset, mesh/primitive/material/texture index ou localizador equivalente quando disponível;
- `action`: próxima correção possível, quando conhecida.

Não usar texto livre de warning como API. Consumidores devem ramificar pelo código, severidade, status e campos tipados.

### 4.3 Fases de validação independentes

Não colapsar as seguintes provas em um único booleano:

| Fase | O que comprova | O que não comprova |
|---|---|---|
| Container/structural | Bytes, offsets, referências e estruturas podem ser lidos | Que os campos têm semântica de authoring correta |
| Semantic reopen | O próprio parser reabre e confere os dados que o writer prometeu preservar | Que um runtime externo renderiza o asset |
| Pipeline compatibility | O asset de origem e as decisões do profile são suportados pelo destino escolhido | Que o resultado foi aceito pelo CodeWalker GUI ou jogo |
| External acceptance | Resultado independente no CodeWalker e/ou no runtime explicitamente testado | Não deve ser inferido por testes internos |

GLB pode ser um arquivo GLB estruturalmente válido e ainda ser incompatível com o exporter. YDR pode passar no semantic reopen e continuar sem runtime acceptance. `runtimeRenderable` só pode mudar com evidência externa correspondente ao alvo (por exemplo FiveM); GTA V Legacy standalone permanece WAIVED/NOT RUN pela decisão anterior do usuário.

### 4.4 Operation context: progresso e cancelamento

Operações longas não devem depender de `println!`, de uma janela Tauri ou de um runtime async específico. A camada de aplicação deve aceitar um contexto opcional com:
- callback/sink de progresso tipado (`operationId`, etapa, completed, total e mensagem curta);
- token/flag cooperativa de cancelamento verificada em loops de decode, conversão, batch e I/O;
- limites de recursos (bytes de entrada, dimensões, pixels, geometria, memória e tempo quando mensurável).

O serviço continua síncrono quando isso for adequado; adapters podem executá-lo em worker thread. CLI converte eventos em stderr (texto ou JSONL). Tauri converte os mesmos eventos em eventos de progresso da aplicação. Nunca misturar progresso com o documento JSON final em stdout.

Não adicionar Tokio ou um sistema de jobs distribuídos apenas para obter progresso local.

## 5. Contrato da CLI

### 5.1 Envelope atual v1

O envelope público existente `ragelab.cli.response` / `schemaVersion: 1` permanece compatível:

Sucesso:
```json
{
  "schema": "ragelab.cli.response",
  "schemaVersion": 1,
  "ok": true,
  "command": "glb.inspect",
  "data": {}
}
```

Erro:
```json
{
  "schema": "ragelab.cli.response",
  "schemaVersion": 1,
  "ok": false,
  "command": "glb.inspect",
  "error": {
    "code": "validation_failed",
    "message": "..."
  }
}
```

O schema JSON de referência para o envelope v1 está em [schemas/ragelab.cli.response.v1.schema.json](schemas/ragelab.cli.response.v1.schema.json). Os schemas dedicados de `version`, capabilities e payloads de domínio continuam independentes.

Códigos de processo existentes são mantidos:
- `0`: sucesso;
- `1`: falha operacional;
- `2`: comando/argumento inválido;
- `3`: operação ou formato não suportado;
- `4`: falha de validação/parsing.

Os identificadores de erro atuais (`operation_failed`, `invalid_input`, `unsupported`, `validation_failed`) não podem mudar de significado.

### 5.2 Evolução do JSON

Não adicionar silenciosamente campos obrigatórios nem mudar a forma do envelope v1. O próximo envelope terá `schemaVersion: 2` e deverá incluir, no mínimo:
- `operationId`;
- `tool` com versão e Core API version;
- `status`;
- `data` em sucesso;
- `diagnostics[]`;
- `error` tipado em falhas;
- `timing`/metrics opcionais.

Durante a transição, v1 continua disponível; v2 será opt-in (`--json-version 2`) até os consumidores serem migrados. A escolha de versão deve estar refletida em `capabilities`. A versão default só muda em uma transição de compatibilidade anunciada.

### 5.3 Gramática e comportamento

A forma namespaced é a forma canônica para novos comandos. Os aliases flat existentes permanecem durante o período 0.x. A CLI deve ter ajuda por nível (`ragelab --help`, `ragelab glb --help`, `ragelab glb export --help`) e descobrir comandos/capabilities da mesma fonte de registro, em vez de manter listas contraditórias.

Semântica das opções:
- `--json`: um único documento JSON em stdout; nunca mistura logs, progresso ou warnings textuais;
- `--strict`: aplica uma política documentada para warnings e features não suportadas; não pode alterar silenciosamente o significado do profile;
- `--overwrite`: opt-in explícito, nunca default;
- `--dry-run`: só aparece em operações que conseguem calcular um plano real sem escrever;
- `--progress text|jsonl|none`: progresso vai para stderr; `jsonl` emite eventos individuais e não substitui o relatório final;
- ausência de opção desconhecida deve gerar erro estruturado quando `--json` for solicitado.

A resposta de `capabilities` deve informar comando canônico, aliases, versão do payload, suporte por formato/operação, requisitos, se há escrita, se há output estruturado e se a disponibilidade depende de parâmetros/contexto.

## 6. Contrato de integração do Studio

1. O backend Tauri chama diretamente as APIs públicas do `ragelab-engine`; o Studio não executa o binário CLI como subprocesso.
2. DTOs de requests/responses usados por Tauri devem ser os mesmos tipos serializáveis do engine sempre que fizer sentido. DTOs próprios de UI são permitidos apenas para interação/viewport e devem converter para tipos Core em fronteira explícita.
3. Erros não devem ser reduzidos a `String` nos novos endpoints. O adapter deve serializar `CoreError` preservando `code`, categoria, diagnostics e retryability. Endpoints existentes com `Result<_, String>` podem ser migrados incrementalmente.
4. Tipos TypeScript em `src/lib/native.ts` não devem evoluir independentemente dos DTOs Rust. Gerar schemas/types dos tipos Rust ou validar os DTOs contra schemas emitidos pelo Core. A estratégia de geração deve suportar Serde rename, optional fields e tagged enums.
5. O Studio deve consultar `coreApiVersion`, `corePackageVersion`, `buildRevision` e capabilities no startup. A revisão Git é metadata; a decisão de compatibilidade usa a API/schema version.
6. Arquivos, paths absolutos, dialogs, cache de instalação e permissões de desktop continuam sob responsabilidade do adapter Tauri. Regras de parser, formatos, material, shader, writer e validation permanecem no Core.
7. `ragelab-render` continua sendo o serviço nativo de rendering que consome contratos do engine; não deve incorporar parsers de arquivos nem authoring de materiais.

### Fluxo alvo: GLB import no Studio

```text
Svelte: usuário escolhe GLB/profile/template/output
  -> Tauri: valida paths e lê bytes
  -> ragelab-engine: inspect/validate/export use case
  -> ragelab-engine: retorna report + output bytes
  -> Tauri: grava output via política non-destructive compartilhada
  -> Svelte: apresenta diagnostics, report e estado de validação
```

O mesmo use case é chamado pela CLI. Apenas a adaptação de entrada/saída muda.

## 7. Versionamento e compatibilidade

Manter separadas:
- versão do pacote/execução CLI;
- versão da API pública Rust do engine;
- versão de cada DTO/schema serializado;
- versão de profiles e manifests persistidos;
- versão de formatos/binários de destino (ex.: YDR v165/RSC7);
- versão do contrato de render package.

Adicionar campos opcionais é permitido quando consumidores existentes continuam válidos. Remover campo, reinterpretar enum/flag, mudar unidade/ordenação ou alterar default exige nova versão de contrato. Mensagens humanas podem melhorar sem bump; códigos e estados estruturados não.

A versão/build deve ser gerada a partir do artefato compilado. Não duplicar strings de revision em vários arquivos nem assumir que Core e Studio apontam para o mesmo HEAD apenas porque usam path dependencies.

## 8. Segurança, determinismo e recursos

- Os bytes de entrada e templates são imutáveis durante a operação.
- Profile e template devem estar associados à identidade/hash da origem, quando aplicável.
- Todas as decisões de shader são verificadas por assinatura estrita e parâmetros/samplers; nome de shader isoladamente não é prova.
- Limites de bytes, decompression ratio, dimensions, texture pixels, vertices, indices, children, nesting e memória devem ser verificados antes de alocar/expandir.
- Batch faz preflight global, detecta colisões e só então escreve; falha por item não pode contaminar outro item.
- Ordenação de relatórios/arrays derivada de mapas deve ser determinística.
- Paths retornados em relatórios são metadata; não devem vazar paths privados desnecessários ou secrets. Nenhum token, conteúdo de .env ou segredo entra em diagnóstico.
- Nenhuma operação toca GTA/FiveM instalado ou modifica RPF original implicitamente. Fontes de referência continuam read-only.

## 9. Ordem de implementação derivada desta decisão

1. ✅ Migrado: o use case GLB→YDR vive em `ragelab-engine`, coordenando `ragelab-gltf`, `ragelab-yft`, `ragelab-ytd` e `ragelab-ydr`; a CLI permanece como adapter compatível.
2. ✅ Migrado: o Studio chama a mesma API por comandos Tauri; a revisão e o estado dirty vêm de build metadata, e a compatibilidade usa `CORE_API_COMPATIBILITY_VERSION` explícita.
3. ✅ DTOs GLB Rust→TypeScript são gerados por `ts-rs`; `bun run generate:glb-types` regenera os arquivos e o teste Tauri verifica o gerador.
4. Implementar `glb validate` como relatório de structural validity versus YDR export compatibility, reaproveitando as mesmas regras do export.
5. Implementar `ydr inspect/validate` usando `ragelab-ydr` e validações semânticas; não criar parser duplicado no engine.
6. Unificar diagnósticos e CoreError; migrar endpoints/CLI gradualmente para envelope v2.
3. Implementar `glb validate` como relatório de structural validity versus YDR export compatibility, reaproveitando as mesmas regras do export.
4. Implementar `ydr inspect/validate` usando `ragelab-ydr` e validações semânticas; não criar parser duplicado no engine.
5. Unificar diagnósticos e CoreError; migrar endpoints/CLI gradualmente para envelope v2.
6. Expandir relatórios de exportação, batch, doctor/capabilities e demais formatos conforme a matriz técnica de cobertura.

## 10. Critérios para encerrar esta arquitetura

- A fronteira CLI/Core/Studio está documentada e coincide com os crates existentes.
- O GLB→YDR está explicitamente identificado como o primeiro fluxo a migrar para o engine; não existe proposta de novo facade crate genérico.
- O envelope CLI v1 e os exit codes existentes têm schema/contrato verificável.
- Os contratos futuros de CoreError, Diagnostic, OperationReport, progress/cancellation e Core API compatibility têm campos e semântica definidos.
- O Studio consome Core diretamente; não há subprocesso CLI nem regra de formato duplicada.
- Testes de contrato atuais do CLI são executados e permanecem verdes; os novos use cases devem ter testes Rust do engine, testes CLI end-to-end e testes Tauri/DTO, todos sobre o mesmo resultado.
