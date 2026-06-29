import type { ValidationError, ValidationWarning } from "../lib/validate";

interface Props {
  errors: ValidationError[];
  warnings: ValidationWarning[];
}

export function ValidationPanel({ errors, warnings }: Props) {
  return (
    <div className="h-full">
      {errors.length === 0 && warnings.length === 0 ? (
        <div className="flex h-full items-center justify-center text-center">
          <span className="text-sm font-medium" style={{ color: "var(--color-ar-in)" }}>
            ✓ 設定は有効です
          </span>
        </div>
      ) : (
        <div className="space-y-3">
          {/* Errors */}
          {errors.length > 0 && (
            <div>
              <p
                className="mb-1.5 text-xs font-semibold"
                style={{ color: "var(--color-destructive)" }}
              >
                Errors ({errors.length})
              </p>
              <ul className="space-y-1">
                {errors.map((e, i) => (
                  <li
                    key={i}
                    className="flex items-start gap-2 rounded-md p-2 text-xs"
                    style={{
                      background: "color-mix(in oklch, var(--color-destructive) 10%, transparent)",
                    }}
                  >
                    <span className="shrink-0" style={{ color: "var(--color-destructive)" }}>
                      ✕
                    </span>
                    <code className="shrink-0 font-mono text-[var(--color-muted-foreground)]">
                      {e.path}
                    </code>
                    <span className="text-[var(--color-foreground)]">{e.message}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {/* Warnings */}
          {warnings.length > 0 && (
            <div>
              <p className="mb-1.5 text-xs font-semibold" style={{ color: "var(--color-ar-gain)" }}>
                Warnings ({warnings.length})
              </p>
              <ul className="space-y-1">
                {warnings.map((w, i) => (
                  <li
                    key={i}
                    className="flex items-start gap-2 rounded-md p-2 text-xs"
                    style={{
                      background: "color-mix(in oklch, var(--color-ar-gain) 10%, transparent)",
                    }}
                  >
                    <span className="shrink-0" style={{ color: "var(--color-ar-gain)" }}>
                      ⚠
                    </span>
                    <code className="shrink-0 font-mono text-[var(--color-muted-foreground)]">
                      {w.path}
                    </code>
                    <span className="text-[var(--color-foreground)]">{w.message}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
