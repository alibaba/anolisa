# Token-Less Extension Context

## TOON Format Reference

TOON is a compact notation for structured data that replaces JSON syntax overhead.
When you see `[tokenless]` annotations with TOON-encoded content, parse as follows:

### Key-Value Pairs
```
name:Alice age:30 city:Beijing
```
Each `key:value` pair maps directly to a JSON field.

### Tabular Data
```
users[3]{id,name,email}:
1|Alice|alice@example.com
2|Bob|bob@example.com
3|Carol|carol@example.com
```
Format: `collection[count]{fields}:` header, then pipe-delimited rows.

### Nested Objects
```
config{debug:false,timeout:30,limits{max:100,min:10}}
```
Braces nest like JSON objects.

### Arrays
```
tags[3]:deploy|staging|prod
```
Pipe-delimited values inside `[count]:` header.

## Compression Behavior

The tokenless extension automatically compresses tool responses:

- **Debug/null/empty fields** are stripped (debug, trace, stack, logs, null values)
- **Long strings** (>512 chars) are truncated with `[...truncated]` markers
- **Large arrays** (>16 items) keep first 8 + last 8 with `[...N items truncated]` marker
- **Content-retrieval tools** (Read, Glob, NotebookRead) are never compressed
- **Skill files** (YAML frontmatter markdown) are never compressed
- **Small responses** (<200 bytes) pass through unchanged

## Annotations

`[tokenless] Tool → label (N% savings)` — indicates compression was applied.
Labels: "response compressed", "TOON encoded", "response compressed + TOON encoded", "passed through"