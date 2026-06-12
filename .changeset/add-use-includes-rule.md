---
"@biomejs/biome": patch
---

Added [`useIncludes`](https://biomejs.dev/linter/rules/use-includes/) to the nursery group. This rule flags comparisons of `String.prototype.indexOf()` / `lastIndexOf()` or `Array.prototype.indexOf()` / `lastIndexOf()` against `-1`, simple `Array#some()` equality checks, and suggests replacing them with the clearer `includes()` / `!includes()` form.
