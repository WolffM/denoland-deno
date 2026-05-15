# feat(fmt): infer config from .editorconfig

## Steps to reproduce

1. Create a directory with a `.editorconfig` file that sets `indent_style = tab`
   and `indent_size = 4`.
2. Create a `deno.json` with no `fmt` options (empty `{}`).
3. Write a TypeScript file that uses **tab** indentation.
4. Run `deno fmt --check <file>` from that directory.

Without this feature, Deno ignores the `.editorconfig` file and defaults to
2-space indentation, causing `deno fmt --check` to report the file as not
formatted even though it matches the project's editorconfig settings.

## Observed

Before implementing this feature, running:

```
deno fmt --check main.ts
```

where `main.ts` uses tab indentation and `.editorconfig` declares
`indent_style = tab` produces:

```
from ./main.ts:
2 | -	return "world";
2 | +    return "world";

error: Found 1 not formatted file in 1 file
```

Deno treats the tab-indented file as incorrectly formatted because it applies
its default (2-space) indentation rule, ignoring the `.editorconfig` file
entirely.

## Expected

After implementing `.editorconfig` inference, running the same command should
output:

```
Checked 1 file
```

Deno should read `indent_style`, `indent_size`, and `max_line_length` from the
nearest `.editorconfig` file and use them as fallback defaults when those
options are not explicitly set in `deno.json` or passed as CLI flags. This
matches the behaviour of Prettier, which also reads `.editorconfig` as a
source of default formatting options.
