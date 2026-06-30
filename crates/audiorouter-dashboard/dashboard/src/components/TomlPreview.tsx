import { useMemo, useState } from "react";

interface Props {
  toml: string;
}

export function TomlPreview({ toml }: Props) {
  const [copied, setCopied] = useState(false);
  const tokens = useMemo(() => tokenizeToml(toml), [toml]);

  const handleCopy = () => {
    void navigator.clipboard.writeText(toml);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="relative h-full min-h-0">
      <button
        type="button"
        onClick={handleCopy}
        aria-label={copied ? "コピーしました" : "TOML をコピー"}
        title={copied ? "Copied" : "Copy"}
        className="absolute top-2 right-2 z-10 flex h-7 w-7 items-center justify-center rounded-md border border-[var(--color-border)] bg-[var(--color-card)]/90 text-xs text-[var(--color-muted-foreground)] shadow-sm backdrop-blur transition hover:bg-[var(--color-muted)] hover:text-[var(--color-foreground)]"
      >
        {copied ? <CheckIcon /> : <CopyIcon />}
      </button>
      <pre className="h-full overflow-auto rounded-md bg-[var(--color-background)] p-3 pr-12 font-mono text-xs leading-relaxed">
        {tokens.map((t, i) => (
          <span key={i} style={{ color: COLOR_MAP[t.type] }}>
            {t.text}
          </span>
        ))}
      </pre>
    </div>
  );
}

// ── TOML tokenizer ────────────────────────────────────────
type TokenType =
  | "table"
  | "key"
  | "string"
  | "number"
  | "boolean"
  | "punctuation"
  | "comment"
  | "text";

interface Token {
  text: string;
  type: TokenType;
}

const COLOR_MAP: Record<TokenType, string> = {
  table: "var(--color-ar-border)", // Cyan — section headers
  key: "var(--color-ar-route)", // LightBlue — keys
  string: "var(--color-ar-in)", // Green — string values
  number: "var(--color-ar-gain)", // Yellow — numeric values
  boolean: "var(--color-ar-out)", // Magenta — booleans
  punctuation: "var(--color-muted-foreground)", // dim — = [ ] ,
  comment: "var(--color-muted-foreground)", // dim — comments
  text: "var(--color-foreground)", // default
};

function tokenizeToml(toml: string): Token[] {
  const lines = toml.split("\n");
  const tokens: Token[] = [];
  for (let li = 0; li < lines.length; li++) {
    tokens.push(...tokenizeLine(lines[li]));
    if (li < lines.length - 1) {
      tokens.push({ text: "\n", type: "text" });
    }
  }
  return tokens;
}

function tokenizeLine(line: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;

  while (i < line.length) {
    const rest = line.slice(i);

    // Whitespace
    const ws = rest.match(/^\s+/);
    if (ws) {
      tokens.push({ text: ws[0], type: "text" });
      i += ws[0].length;
      continue;
    }

    // Comment
    if (rest.startsWith("#")) {
      tokens.push({ text: rest, type: "comment" });
      break;
    }

    // Table header: [section] or [[array]]
    const table = rest.match(/^\[+\s*[\w.-]+\s*\]+/);
    if (table) {
      tokens.push({ text: table[0], type: "table" });
      i += table[0].length;
      continue;
    }

    // String ("..." or '...')
    const str = rest.match(/^"(?:\\.|[^"\\])*"|^'(?:[^'])*'/);
    if (str) {
      tokens.push({ text: str[0], type: "string" });
      i += str[0].length;
      continue;
    }

    // Boolean
    const bool = rest.match(/^(?:true|false)\b/);
    if (bool) {
      tokens.push({ text: bool[0], type: "boolean" });
      i += bool[0].length;
      continue;
    }

    // Key (bare word followed by '=')
    const key = rest.match(/^[\w-]+(?=\s*=)/);
    if (key) {
      tokens.push({ text: key[0], type: "key" });
      i += key[0].length;
      continue;
    }

    // Number
    const num = rest.match(/^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/);
    if (num) {
      tokens.push({ text: num[0], type: "number" });
      i += num[0].length;
      continue;
    }

    // Punctuation
    if ("=[],".includes(rest[0])) {
      tokens.push({ text: rest[0], type: "punctuation" });
      i += 1;
      continue;
    }

    // Fallback: single character
    tokens.push({ text: rest[0], type: "text" });
    i += 1;
  }

  return tokens;
}

function CopyIcon() {
  return (
    <svg
      aria-hidden="true"
      className="h-3.5 w-3.5"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="1.5"
    >
      <rect x="5" y="5" width="8" height="8" rx="1.5" />
      <path d="M3 10.5V4.5A1.5 1.5 0 0 1 4.5 3H10.5" />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg
      aria-hidden="true"
      className="h-3.5 w-3.5"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="1.8"
    >
      <path d="M3.5 8.5L6.5 11.5L12.5 4.5" />
    </svg>
  );
}
