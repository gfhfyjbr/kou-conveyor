# OpenRouter Fusion: как устроен и как его повторить

*Исследование от 2026-10-09. Источники перечислены в конце. Что взято из документации и OpenAPI-спеки,
а что выведено мной, помечено.*

## Коротко

- **Fusion — это серверный инструмент `openrouter:fusion`, а не модель.** Обычная («внешняя») модель
  сама решает, вызывать ли его. Инструмент отправляет промпт **панели** из 1–8 моделей. Они отвечают
  параллельно, и у каждой есть `web_search`/`web_fetch`. Затем **аналитик** (judge) при temperature 0
  **сравнивает** ответы, а не сливает их, и возвращает JSON с полями `consensus`, `contradictions`,
  `partial_coverage`, `unique_insights` и `blind_spots`. Финальный ответ пишет внешняя модель: у неё
  есть и анализ, и сырые ответы панели.
- `openrouter/fusion` и `openrouter/fusion-flash` — это псевдонимы-«модели». Роутер подставляет
  реальную модель и подключает ей этот инструмент.
- По сути это однослойный **Mixture-of-Agents**. Отличий три:
  1. судья выдаёт структуру, а не текст;
  2. синтез делает вызывающая модель;
  3. дорогой шаг включается только тогда, когда он нужен.
- **Где выигрыш.** На бенчмарке DRACO (100 задач deep research):
  - панель Fable 5 + GPT‑5.5 набирает **69.0%**, лучшая одиночная модель — 65.3%;
  - бюджетная панель (Gemini 3 Flash + Kimi K2.6 + DeepSeek V4 Pro) набирает 64.7% — в пределах
    1 п.п. от одиночного Fable 5 и, по словам OpenRouter, за 50% его стоимости;
  - **Opus 4.8 в паре с самим собой — 65.5%** против 58.8% у одиночного Opus.

  Значит, заметная часть прироста идёт от дополнительного test-time compute и шага синтеза, а не от
  разнообразия моделей. Это сходится с работой Self-MoA.
- **Цена.** Платите за N вызовов панели, вызов аналитика и внешний вызов. С панелью из трёх моделей
  это примерно 4–5× стоимости одного ответа. По словам OpenRouter, когда Fusion срабатывает, ответ идёт
  в 2–3 раза дольше. Независимый замер с HN: в 7 раз медленнее и в 4 раза дороже.
- **В нашем harness Fusion сейчас не заработает.** Декодер Responses API падает на элементе вывода
  `openrouter:fusion`. Я проверил это пробным тестом через `go test -overlay`, файлы репозитория не
  менялись. Ошибка: `output item 0: unsupported output item type "openrouter:fusion"`.
- **Повторить можно тремя путями** (раздел 4). Рекомендую такой порядок:
  1. однострочный фикс декодера, чтобы попробовать нативный `openrouter/fusion-flash`;
  2. свой `Fusion` в виде плагина kou-conveyor — он не зависит от провайдера внешней модели и даёт
     полный контроль над промптами;
  3. позже, если понадобится, — встроенный инструмент, где у панели есть доступ к коду только на чтение.

## 1. Как это устроено у OpenRouter

### 1.1. Как включить

| Способ | Запрос | Что происходит |
|---|---|---|
| Модель-алиас | `"model": "openrouter/fusion"` | роутер выбирает реальную модель и сам подключает ей `openrouter:fusion` |
| Быстрый пресет | `"model": "openrouter/fusion-flash"` | то же с пресетом `general-fast` («latency-homogeneous panel tuned for fast agentic turns») |
| Явный серверный инструмент | любая модель + `"tools": [{"type": "openrouter:fusion", "parameters": {...}}]` | внешнюю модель выбираете сами, Fusion можно сочетать с другими инструментами |
| Конфиг | `"plugins": [{"id": "fusion", "preset": "general-budget"}]` | только настраивает запуск; сам по себе Fusion не включает |

Пресеты в спеке заданы перечислением: `general-high` (Quality), `general-budget`, `general-fast`.
Пресет «expands server-side into the preset's analysis_models panel and analyst model». Явно
переданные `analysis_models`/`model` важнее пресета. Quality по умолчанию состоит из
`~anthropic/claude-opus-latest`, `~openai/gpt-sol-latest` и `~google/gemini-pro-latest`.

Модель вызывает инструмент сама: в его описании сказано делать это только там, где несколько точек
зрения действительно помогают. Чтобы вызывать его всегда, ставят `tool_choice: "required"`.

### 1.2. Конвейер

```
запрос ─► внешняя модель ──(tool call openrouter:fusion)──┐
                                                          ▼
              ┌──────────── панель: до 8 моделей параллельно ─────────────┐
              │ каждая: user prompt + web_search/web_fetch,                │
              │ ≤ max_tool_calls шагов (по умолчанию 4), ≤ 16000 токенов    │
              └──────────────────────────┬────────────────────────────────┘
                                         ▼
          аналитик (по умолчанию = внешняя модель), temperature 0, + веб-инструменты
          → JSON {consensus, contradictions, partial_coverage, unique_insights, blind_spots}
                                         ▼
          результат инструмента {status, analysis, responses[], failed_models[], sources[]}
                                         ▼
                      внешняя модель пишет финальный ответ
```

Решения, которые стоит скопировать:

1. **Fusion — инструмент, а не модель.** Рутину базовая модель делает сама, а за совещание платят
   только на сложных вопросах. В FAQ это прямо советуют для кодинга: «The base model handles routine
   coding directly and can choose to call Fusion selectively».
2. **Аналитик сравнивает, а не сливает.** Голосования большинством нет. Согласие считается более
   надёжным, уникальные находки сохраняются, слепые зоны помечаются. Текст пишет внешняя модель,
   поэтому на стадии судьи ничего не теряется.
3. **При сбоях результат деградирует, а не падает** (см. 1.4).
4. **Всё ограничено:**
   - панель — не больше 8 моделей;
   - `max_tool_calls` — от 1 до 16;
   - `max_completion_tokens` задаётся на каждый внутренний вызов;
   - за один ход — не больше одного Fusion (иначе `fusion_invocation_capped`);
   - от рекурсии защищают заголовок `x-openrouter-fusion-depth` и запрет подключать инструмент
     повторно: панель и аналитик не могут сами вызвать Fusion.

### 1.3. Параметры (`tools[].parameters` или `plugins[]`)

| Поле | По умолчанию | Смысл |
|---|---|---|
| `analysis_models` | пресет Quality | от 1 до 8 моделей панели («Capped at 8 models to bound cost amplification») |
| `model` | внешняя модель | аналитик |
| `max_tool_calls` | 4 (от 1 до 16) | сколько шагов веб-цикла может сделать каждый участник панели и аналитик |
| `max_completion_tokens` | 16000 | на каждый внутренний вызов, включая reasoning |
| `reasoning` | по умолчанию провайдера | `{effort, max_tokens}` для панели и аналитика |
| `temperature` | по умолчанию провайдера | только для панели; аналитик всегда работает при 0 |
| `tools` | `[web_search, web_fetch]` | серверные инструменты панели и аналитика; `[]` отключает их, и ответы идут из знаний модели; можно задать `excluded_domains` |
| `preset` (только в plugins) | — | `general-high` / `general-budget` / `general-fast` |

### 1.4. Что возвращает инструмент

```json
{
  "status": "ok",
  "analysis": {
    "consensus": ["..."],
    "contradictions": [{"topic": "...", "stances": [{"model": "openai/gpt-5", "stance": "..."}]}],
    "partial_coverage": [{"models": ["openai/gpt-5"], "point": "..."}],
    "unique_insights": [{"model": "anthropic/claude-sonnet-4.5", "insight": "..."}],
    "blind_spots": ["..."]
  },
  "responses": [{"model": "...", "content": "полный ответ участника"}],
  "failed_models": [{"model": "...", "error": "...", "status_code": 429}],
  "sources": [{"url": "...", "title": "..."}]
}
```

- **Часть панели упала.** Если ответил хотя бы один участник, возвращается `status: "ok"` и список
  `failed_models`.
- **Упал аналитик** (ошибка upstream, пустой ответ или невалидный JSON). Тоже `status: "ok"`: сырые
  `responses` есть, а поля `analysis` нет.
- **Жёсткий отказ** — `status: "error"` и `failure_reason`. Возможные значения: `all_panels_failed`,
  `insufficient_credits`, `rate_limited`, `fusion_invocation_capped`, `unexpected_error`. В спеке
  перечислены ещё `invalid_model`, `judge_not_valid_json`, `judge_schema_mismatch`,
  `judge_upstream_error`, `judge_empty_completion` и `cancelled`.

### 1.5. Формат на проводе (Responses API)

- В `output` появляется элемент `{"type": "openrouter:fusion", "id": "st_…", "status": …}` с полями
  из 1.4. После него идёт обычный `message` внешней модели.
- Порядок событий в стриме:
  1. `response.fusion_call.in_progress`;
  2. `.panel.added` (model);
  3. `.panel.delta` и `.panel.reasoning.delta`;
  4. `.panel.completed` (model, content) или `.panel.failed` (model, error, status_code);
  5. `.analysis.in_progress`;
  6. `.analysis.completed` (analysis);
  7. `response.fusion_call.completed`.
- **Биллинг.** В `/api/v1/models` у `openrouter/fusion` цена `-1`, то есть переменная. Вы платите за
  внутренние генерации. В метаданных генерации стоит `router: "openrouter/fusion"`.
- **Приватность.** Внутренние генерации идут под вашим ключом, и настройки ZDR аккаунта на них
  действуют. Но `provider`-предпочтения из запроса внутрь не передаются.

### 1.6. Чего нет в открытом доступе

- Системных промптов панели и аналитика.
- Схемы аргументов вызова инструмента. По спеке каждый участник «receives the user prompt»; получает
  ли он всю переписку или только последний промпт, не уточняется. У элемента вывода нет поля с
  аргументами.
- Точного состава `general-budget` и `general-fast`. В блоге бюджетная панель — это Gemini 3 Flash,
  Kimi K2.6 и DeepSeek V4 Pro.
- Того, как от аналитика добиваются JSON. *(вывод)* Раз есть коды `judge_not_valid_json` и
  `judge_schema_mismatch`, ответ, видимо, разбирают и проверяют уже после генерации. Значит, строгого
  constrained decoding либо нет, либо он есть не для всех моделей.

## 2. Откуда это растёт и что говорят данные

**Mixture-of-Agents** (Wang et al., 2024, arXiv:2406.04692). Есть слои «proposers» и «aggregator» с
промптом Aggregate-and-Synthesize: «synthesize these responses into a single, high-quality
response… critically evaluate… some of it may be biased or incorrect… should not simply replicate».
Fusion — это один такой слой, только агрегатор заменён цепочкой «аналитик → структура → синтез
внешней моделью».

**Self-MoA** (Li et al., 2025, arXiv:2502.00674, TMLR'25):

- смешивание *разных* LLM часто снижает среднее качество;
- несколько сэмплов одной лучшей модели обходят классический MoA: +6.6% на AlpacaEval 2.0 и в среднем
  +3.8% на MMLU, CRUX и MATH;
- результат MoA «rather sensitive to the quality» участников;
- есть последовательный вариант, который агрегирует много сэмплов по раундам.

**karpathy/llm-council** — три стадии:

1. Модели отвечают независимо.
2. **Анонимное** перекрёстное ревью. Ответы помечены «Response A/B/C», каждая модель выдаёт блок
   `FINAL RANKING:`, ранги усредняются.
3. Председатель синтезирует итоговый ответ.

Анонимизация защищает от того, что судья предпочитает свои собственные ответы (self-preference).

**DRACO у OpenRouter.** Это 100 задач deep research от Perplexity. У каждой задачи рубрика примерно из
39 взвешенных критериев, среди них есть отрицательные. Судья — Gemini 3.1 Pro Preview, каждую задачу
оценивали трижды.

| Конфигурация | Балл |
|---|---|
| Fusion: Fable 5 + GPT‑5.5 (синтез Opus 4.8) | **69.0%** |
| Fusion: Opus 4.8 + GPT‑5.5 + Gemini 3.1 Pro | 68.3% |
| Fusion: Opus 4.8 + GPT‑5.5 | 67.6% |
| Fusion: Opus 4.8 + Opus 4.8 | 65.5% |
| Solo: Fable 5 (93 задачи из 100, остальные заблокировали фильтры контента) | 65.3% |
| Fusion: Gemini 3 Flash + Kimi K2.6 + DeepSeek V4 Pro | 64.7% |
| Solo: DeepSeek V4 Pro / GPT‑5.5 / Opus 4.8 | 60.3 / 60.0 / 58.8% |
| Solo: Kimi K2.6 / Gemini 3.1 Pro / Gemini 3 Flash | 53.7 / 45.4 / 43.1% |

Нюанс: с веб-поиском панель находила в сети саму рубрику DRACO. Эти домены пришлось исключить через
`excluded_domains`. Учтите это, если будете гонять свои бенчмарки с веб-инструментами.

**Критика из треда на HN:**

- Судья на деле оценивает, «насколько это похоже на ответ, который дал бы я». Дополнительные раунды
  по сути просто поднимают температуру.
- Польза зависит от того, можно ли проверить результат. Если ответ не проверить, модель-ревьюер может
  навредить.
- В одном замере Fusion оказался в 7 раз медленнее и в 4 раза дороже, чем Opus или GPT напрямую.
- Разные персоны и углы зрения у участников дают больше, чем один и тот же промпт для всех.
- Большая часть «разнообразия» может быть просто шумом сэмплирования: одна модель при повторных
  запусках находит разные подмножества багов.

**Что из этого следует для дизайна:**

1. Качество участников важнее их разнообразия. Сильная отправная точка — «self-fusion» лучшей
   доступной модели.
2. Fusion окупается на исследованиях, архитектуре, критике планов и решениях, где ошибка стоит дорого.
   На рутинных правках, классификации и переформатировании — нет.
3. Для задач, которые можно проверить (код с тестами), лучше генерировать N вариантов и отбирать
   по результату запуска, а не через LLM-судью.

## 3. Рецепт реализации (не зависит от платформы)

### 3.1. Алгоритм

```go
// Ядро — около 150 строк. complete() — один вызов LLM, с серверными веб-инструментами или без.
func Fuse(ctx context.Context, cfg Config, prompt string) Result {
	pctx, cancel := context.WithTimeout(ctx, cfg.PanelTimeout) // медленные участники попадут в failed_models
	defer cancel()
	results := make([]Answer, len(cfg.Panel))
	sem := make(chan struct{}, cfg.Concurrency)
	var wg sync.WaitGroup
	for i, member := range cfg.Panel { // member: модель и, при желании, персона или угол зрения
		wg.Add(1)
		go func() {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			text, err := complete(pctx, Call{Model: member.Model, System: panelPrompt(member),
				User: prompt, Tools: cfg.PanelTools, MaxTokens: cfg.MaxTokens, Temperature: cfg.Temperature})
			results[i] = Answer{Model: member.Model, Content: text, Err: err}
		}()
	}
	wg.Wait()

	ok, failed := split(results)
	if len(ok) == 0 {
		return Result{Status: "error", FailureReason: classify(failed), FailedModels: failed}
	}
	res := Result{Status: "ok", Responses: ok, FailedModels: failed}

	labels := anonymize(ok) // "Response A" → модель; защита от self-preference (идея llm-council)
	raw, err := complete(ctx, Call{Model: cfg.Analyst, System: analystPrompt,
		User: render(prompt, ok, labels, cfg.AnalystBudget), Temperature: 0, Schema: analysisSchema})
	if err == nil {
		if analysis, err := parseAndValidate(raw, labels); err == nil { // метки → настоящие слаги моделей
			res.Analysis = &analysis
		}
	}
	return res // если аналитик упал — ok без analysis, как у OpenRouter
}
```

### 3.2. Промпты

Настоящие промпты OpenRouter не опубликованы. Ниже моя реконструкция: она основана на описанном в
документации поведении и на `JUDGE_SYSTEM_PROMPT` из pi-fusion.

Панель:

```
You are an independent expert answering the question below. Answer thoroughly and
accurately; use the available tools to verify facts when it helps, then write a complete
final answer. State uncertainty explicitly. Do not mention other models or a panel.
```

Аналитик:

```
You compare answers that several independent experts gave to the same task. You are
given the task and responses labelled "Response A", "Response B", ... Return only JSON
matching the schema. Compare, do not merge or average:
- consensus: points all or most responses agree on (treated as higher confidence);
- contradictions: real disagreements, each with every response's stance; do not invent them;
- partial_coverage: points only some responses covered, with which ones;
- unique_insights: valuable points raised by exactly one response;
- blind_spots: important aspects of the task that no response addressed.
Judge claims on their merits, not on how closely they match what you would have said.
Where you can verify a claim (tools, arithmetic, logic), do so, and say which side is correct.
```

### 3.3. JSON-схема ответа аналитика (подходит для strict-режима)

```json
{"type":"object","additionalProperties":false,
 "required":["consensus","contradictions","partial_coverage","unique_insights","blind_spots"],
 "properties":{
  "consensus":{"type":"array","items":{"type":"string"}},
  "contradictions":{"type":"array","items":{"type":"object","additionalProperties":false,
    "required":["topic","stances"],"properties":{"topic":{"type":"string"},
    "stances":{"type":"array","items":{"type":"object","additionalProperties":false,
      "required":["model","stance"],"properties":{"model":{"type":"string"},"stance":{"type":"string"}}}}}}},
  "partial_coverage":{"type":"array","items":{"type":"object","additionalProperties":false,
    "required":["models","point"],"properties":{"models":{"type":"array","items":{"type":"string"}},"point":{"type":"string"}}}},
  "unique_insights":{"type":"array","items":{"type":"object","additionalProperties":false,
    "required":["model","insight"],"properties":{"model":{"type":"string"},"insight":{"type":"string"}}}},
  "blind_spots":{"type":"array","items":{"type":"string"}}}}
```

Через OpenRouter схему передают так:
`response_format: {"type":"json_schema","json_schema":{"name":"fusion_analysis","strict":true,"schema":…}}`.
Ответ всё равно проверяйте после получения: не все модели поддерживают structured outputs. Судя по
кодам ошибок, OpenRouter делает именно так.

### 3.4. Что настраивать и чем можно улучшить вариант OpenRouter

- **Лимиты:**
  - панель — не больше 8 моделей;
  - `max_tool_calls`;
  - `max_tokens` на каждый вызов;
  - общий таймаут панели;
  - бюджет в байтах на вход аналитика (pi-fusion обрезает ответы перед тем, как отдать их судье).
- **Защита от рекурсии.** У внутренних вызовов инструмента Fusion нет по построению. Дополнительно
  можно передавать счётчик глубины в заголовке или переменной окружения.
- **Не больше одного Fusion за ход.**
- **Анонимизация для аналитика.** У OpenRouter её, судя по полю `model` в stances, нет. Особенно она
  важна, когда аналитик — та же модель, что сидит в панели.
- **Разнообразие через промпт.** Дайте участникам персоны или углы зрения («security reviewer»,
  «performance», «maintainer»), а не только разные модели.
- **Режим self-fusion:** одна модель N раз при temperature > 0 (как в Self-MoA).
- **Хеджирование задержки:** возвращать результат, как только ответили k из N участников, или по
  таймауту.
- **Кэш промптов.** У всех участников общий префикс, его можно кэшировать через `cache_control`.
- **Учёт стоимости:** суммировать `usage.cost` всех внутренних вызовов.

## 4. Как встроить в kou-conveyor

### 4.0. Что есть сейчас (по коду)

- OpenRouter-клиент ходит в Responses API: `harness/llm/clients/openrouter/client.go`, эндпоинт
  `/responses`. Через `Extensions` клиент добавляет поля верхнего уровня в каждый запрос; сейчас это
  `cache_control`.
- `requestTool` (`harness/llm/responsesapi/request.go:398`) понимает function-инструменты и ровно один
  hosted-инструмент — `web_search`. Вне тестов `llm.ToolHosted` нигде не создаётся.
- `responseOutputItem` (`harness/llm/responsesapi/response.go:76`) пропускает `web_search_call`, а на
  любой другой незнакомый тип возвращает ошибку (`response.go:164`).
  - Я проверил это временным тестом через `go test -overlay`, в репозитории ничего не менялось. Ответ
    с элементом `openrouter:fusion` даёт `output item 0: unsupported output item type
    "openrouter:fusion"`.
  - Значит, если выбрать модель агента `openrouter/fusion` или `fusion-flash`, ход почти наверняка
    сломается при первом реальном вызове Fusion.
- **Цены.** `cmd/internal/accounts/openrouter.go` пропускает алиасы (`~`, `:`) и модели с
  отрицательной ценой. У `openrouter/fusion` цена `-1`, поэтому расход на Fusion по токенам не
  посчитается — нужен `usage.cost` из ответа. Входят ли туда внутренние генерации, не задокументировано;
  это надо проверить на живом вызове.

### Путь A — нативный Fusion от OpenRouter (маленький патч, работает только через OpenRouter)

1. В `response.go` добавить `case "web_search_call", "openrouter:fusion": return llm.Item{}, false, nil`.
   Этого уже хватает, чтобы выбрать модель `openrouter/fusion-flash` или `openrouter/fusion`. Пресет
   задаётся через `Extensions` клиента: `"plugins": [{"id":"fusion","preset":"general-budget"}]`.
2. Для явного инструмента с любой моделью OpenRouter:
   - в `requestTool` добавить hosted-инструмент `openrouter:fusion` →
     `{"type":"openrouter:fusion","parameters":{…}}`; union `openaiapi.Tool` хранит сырой JSON, так что
     своя структура в него маршалится;
   - нужно ещё место, откуда этот hosted-инструмент попадёт в `llm.Request.Tools`: настройка модели
     или пресета.
3. По желанию: в `stream.go` (функция `observe`) превращать события `response.fusion_call.*` в
   прогресс в таймлайне — какой участник добавлен, какой завершился или упал, готов ли анализ.
4. Тесты: рядом с `TestRequestBodyEncodesHostedWebSearch` (`request_test.go:349`) и отдельный тест
   декодирования.

Плюсы: своей логики почти нет, пресеты и веб-инструменты даёт OpenRouter.

Минусы:
- работает только через OpenRouter;
- панель не видит воркспейс — только промпт и веб;
- промпты не настраиваются;
- не больше одного вызова за ход;
- `provider`-предпочтения не пробрасываются;
- при повторе истории элемент `openrouter:fusion` придётся выбрасывать, так что анализ останется
  только в виде финального ответа.

### Путь B — свой `Fusion` в виде плагина (рекомендую как основной)

Плагин кладётся в `.harness/plugins/fusion/`, если он нужен только в этом воркспейсе, или в
`~/Library/Application Support/kou-conveyor/plugins/fusion/`, если во всех.

```json
{
  "name": "fusion",
  "version": "0.1.0",
  "description": "Multi-model deliberation: a panel answers in parallel, an analyst compares",
  "requires": {"env": ["OPENROUTER_API_KEY"]},
  "prompt": "prompt.md",
  "tools": [{
    "name": "Fusion",
    "description": "Ask a panel of different models the same self-contained question in parallel; get back a structured comparison (consensus, contradictions, partial coverage, unique insights, blind spots) and their raw answers. Use only when a wrong answer is expensive: architecture and design trade-offs, research, critique of a plan. Not for routine edits. The panel sees only `prompt` and `context`, not the workspace or this conversation: include the relevant code, constraints and goals.",
    "parameters": {"type": "object", "required": ["prompt"], "properties": {
      "prompt":  {"type": "string", "description": "The self-contained question"},
      "context": {"type": "string", "description": "Relevant excerpts: code, logs, constraints"}}},
    "run": ["./fusion"],
    "max_output_length": 200000
  }]
}
```

- **Программа.** `./fusion` — небольшая программа на Go: только стандартная библиотека, ядро из 3.1.
  Аргументы приходят строкой JSON на stdin, результат в формате из 1.4 печатается в stdout.
- **Конфиг.** Панель, аналитик, лимиты и пресеты лежат в `fusion.json` в `$KOU_CONVEYOR_PLUGIN_DIR`.
  Вызывающая модель не может выбрать или переопределить панель — так же сделано в pi-fusion.
- **Вызовы.** Панель ходит в OpenRouter с
  `tools: [{"type":"openrouter:web_search"},{"type":"openrouter:web_fetch"}]`. Веб-цикл крутит сам
  OpenRouter, так что свой агентный цикл в плагине не нужен. Аналитик вызывается с JSON-схемой и
  temperature 0.
- **Работа в фоне.** По документации плагинов вызов плагина — это durable operation: он идёт в фоне,
  агент тем временем продолжает работать, а после перезапуска раннера вызов возобновляется. Серверный
  инструмент OpenRouter, наоборот, блокирует весь ответ.
- **Размер вывода.** `max_output_length` можно поднять до 1 000 000 (`harness/plugin/plugin.go:103` и
  `:284`). Но разумнее возвращать анализ и урезанные ответы панели.
- **Подсказки модели.** В `prompt.md` — правила, когда вызывать Fusion и как упаковать контекст. Через
  `commands` можно добавить slash-команду `/fusion <вопрос>`.
- **Объём.** Примерно 300–400 строк Go плюс манифест. Менять harness не нужно, плагин работает с любым
  провайдером внешней модели.

### Путь C — встроенный инструмент с панелью-агентами (самый мощный)

- **Где и через что.** Инструмент живёт в `harness/tool/fusion`. Панель вызывается через `llm.Adapter`
  любого настроенного провайдера: openai, anthropic, openai-codex (подписка), openrouter, fireworks,
  ollama. Так можно смешивать модели с собственных аккаунтов; pi-fusion делает то же самое с моделями,
  под которые у пользователя уже есть авторизация.
- **Доступ к коду.** Главное отличие от путей A и B: участникам панели можно дать **инструменты только
  на чтение** — Read, rg, ls — через маленький агентный цикл. Тогда ответы опираются на реальный код,
  а не на пересказ в `context`.
- **Защита от рекурсии.** Набор инструментов панели — жёсткий список разрешённых без Fusion, как в
  pi-fusion.
- **Пишущие инструменты.** Если они всё же нужны, участники работают только по очереди (в pi-fusion
  для этого concurrency=1) или каждый в своём worktree.
- **Тяжёлый вариант через канвас** — «агентная панель», похожая на схемы с ревью и ответами на ревью,
  которые описывали на HN:
  1. `CanvasSpawn` запускает N агентов с разными `config.model`, `access: "observe"` и при
     необходимости своим `worktree`;
  2. `CanvasSend` отправляет всем один и тот же промпт;
  3. `CanvasRead` собирает ответы;
  4. ответы уходят аналитику.

### Рекомендованный порядок

1. **Однострочный фикс декодера** (путь A, шаг 1) и тест к нему. Он нужен в любом случае: иначе выбор
   `openrouter/fusion*` ломает ход. После этого можно сравнить `fusion-flash` с текущей моделью на 3–5
   реальных задачах — архитектурных вопросах, ревью плана. Сравнивать стоит стоимость принятого
   результата, а не цену токена.
2. **Плагин `Fusion`** (путь B). В нём:
   - пресеты: `self` (лучшая модель ×3), `budget`, `quality`;
   - анонимизация ответов для аналитика;
   - лимит `max_tool_calls`.
3. **Путь C** — если выяснится, что панели не хватает доступа к коду.

## Источники

- Fusion Router — https://openrouter.ai/docs/guides/routing/routers/fusion-router
- Серверный инструмент Fusion — https://openrouter.ai/docs/guides/features/server-tools/fusion
- Серверные инструменты (список, ZDR) — https://openrouter.ai/docs/guides/features/server-tools
- OpenAPI для Responses (`OutputFusionServerToolItem`, `FusionAnalysisResult`, `FusionServerToolConfig`,
  `FusionPlugin`, события `response.fusion_call.*`) —
  https://openrouter.ai/docs/api/api-reference/responses/create-a-response.md
- Блог «Surpassing Frontier Performance with Fusion» (DRACO, self-fusion, FAQ) —
  https://openrouter.ai/blog/announcements/fusion-beats-frontier
- Блог «OpenRouter Fusion: How It Works and When to Use It» —
  https://openrouter.ai/blog/insights/fusion-explainer
- Каталог моделей (цена `-1` у `openrouter/fusion`) — https://openrouter.ai/api/v1/models
- Обсуждение на HN — https://news.ycombinator.com/item?id=48537641
- pi-fusion (открытая реимплементация на TypeScript) — https://github.com/synthetic-recon/pi-fusion
- karpathy/llm-council — https://github.com/karpathy/llm-council
- Mixture-of-Agents — https://arxiv.org/abs/2406.04692
- Self-MoA — https://arxiv.org/abs/2502.00674
