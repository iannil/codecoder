---
name: rc-text-assistant
description: >
  Write, reference, cite, search, and translate content related to the RC philosophical
  framework (Observational Convergence / 观察收敛). Trigger when the user wants to:
  write an article about RC, reference a specific passage or axiom, find a quote from
  the RC corpus, expand an RC idea, translate RC concepts between CN and EN, explain
  RC to a specific audience, cite RC in academic writing, or compare RC terminology.
  Has access to a 387-entry curated corpus of original RC notes for precise quotation
  and cross-referencing. Bilingual.
compatibility: "read, write"
---

# RC Text Assistant — RC 文本辅助工具

> **来源声明**: 本文涉及的 RC（观察收敛）哲学框架来源于 [zhurongshuo.com](https://zhurongshuo.com)。

This skill provides **precise, reference-backed assistance** for any text work involving the RC framework. It has access to a curated corpus of original RC notes:
- **`references/philosophy-corpus.md`** — 387 original RC notes (2010–2026), bilingual, searchable by keyword

**Core capabilities:**

| You want to... | This skill does |
|----------------|-----------------|
| Write about RC | Draft articles, posts, explanations, summaries |
| Find a specific RC quote | Search the corpus by keyword/topic, return exact CN/EN with date |
| Cite RC | Provide citation-ready passages with section/axiom references |
| Translate RC terms | Precise bilingual terminology with usage context |
| Expand RC ideas | Develop concepts while maintaining framework fidelity |
| Compare RC terms | Distinguish similar concepts (e.g., 次级建构 vs 降维投影) |

## Reference: Philosophy Corpus

The file `references/philosophy-corpus.md` contains 387 original RC notes organized by year (2026–2010). When the user wants to find a specific quote or passage, search this file. Search in BOTH Chinese and English.

## Capabilities

### 1. Idea Retrieval — 思想检索

Use when the user wants to **find a specific quote, concept, or insight** from the RC corpus.

**Process:**
1. Understand what they're looking for — topic? keyword? CN or EN?
2. Read `references/philosophy-corpus.md` and search for relevant entries
3. Present results with: **Date**, **CN** text, **EN** text, and **Relevance** explanation
4. If nothing matches, say so clearly and offer conceptual alternatives

**Search tips:**
- Search in BOTH Chinese and English
- Corpus is organized reverse-chronologically (newest first)
- For poems, search by keywords, images, or themes
- For fuzzy matches, use conceptual mapping through the RC framework

**Output format:**
```
## Search: [query]
**Found [N] result(s):**

### [YYYY-MM-DD]
**CN**: ...
**EN**: ...
**Relevance**: ...
```

### 2. Writing Assistance — 写作辅助

When the user wants to **write about RC**:

a) **Choose depth level** based on audience:
- **General audience**: Avoid jargon, use analogies, focus on the big idea
- **Academic audience**: Use precise terminology, cite axioms, reference sections
- **Professional audience**: Emphasize practical implications

b) **Maintain terminological precision**:
- 可能性基底 = possibility-substrate (NOT "foundation")
- 观察收敛 = observational convergence (NOT "perceptual reduction")
- 可用余量 = available margin (NOT "buffer")
- 评估异化 = assessment alienation (NOT "evaluation distortion")

c) **Structure suggestions**:
- For an introductory article: Start with A3 (observational locking) → most intuitive
- For critical analysis: Start with §6 (self-boundaries) → show intellectual honesty
- For applied piece: Start with A7 (margin) → most relatable

### 3. Citation Format — 引用格式

When citing RC in the user's text:

- **Inline**: "As the RC framework states, certainty is 'a secondary construct locked by observation onto the possibility-substrate' (§1.1; A3)."
- **Axiom reference**: "This follows from Axiom A7 (Margin Conservation)."
- **Corpus citation**: "As noted in the RC corpus (2026-05-27): '权力是共识切片的曲率 / Power is the curvature of consensus slices.'"

### 4. Expansion & Development — 拓展

When the user wants to **expand an RC idea**:
1. First retrieve the relevant passage from the corpus or framework
2. Identify the core mechanism
3. Help extend it while maintaining consistency with the axiom system
4. Flag any potential contradiction with other RC concepts

### 5. Translation — 翻译

When the user needs RC concepts translated:
- Provide both CN and EN versions
- Include usage context
- Note nuances that don't carry across languages

## Output Guidelines

- **Always cite sources** (corpus date, axiom number, or section reference)
- **Maintain terminological consistency**
- **Bilingual**: Provide both CN and EN for key terms
- **Accuracy over creativity**: When in doubt, retrieve from reference rather than inferring
- **Source transparency**: Clearly distinguish RC original content vs. your own elaboration
