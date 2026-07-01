import type { ValidationError, ValidationWarning } from "../lib/api";

type ValidationIssue = ValidationError | ValidationWarning;

interface Props {
  errors: ValidationError[];
  warnings: ValidationWarning[];
  onIssueClick?: (issue: ValidationIssue) => void;
}

export function ValidationPanel({ errors, warnings, onIssueClick }: Props) {
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
          {errors.length > 0 && (
            <IssueGroup
              title={`Errors (${errors.length})`}
              toneColor="var(--color-destructive)"
              icon="✕"
              issues={errors}
              onIssueClick={onIssueClick}
            />
          )}

          {warnings.length > 0 && (
            <IssueGroup
              title={`Warnings (${warnings.length})`}
              toneColor="var(--color-ar-gain)"
              icon="⚠"
              issues={warnings}
              onIssueClick={onIssueClick}
            />
          )}
        </div>
      )}
    </div>
  );
}

function IssueGroup({
  title,
  toneColor,
  icon,
  issues,
  onIssueClick,
}: {
  title: string;
  toneColor: string;
  icon: string;
  issues: ValidationIssue[];
  onIssueClick?: (issue: ValidationIssue) => void;
}) {
  return (
    <div>
      <p className="mb-1.5 text-xs font-semibold" style={{ color: toneColor }}>
        {title}
      </p>
      <ul className="space-y-1">
        {issues.map((issue, i) => (
          <li key={`${issue.path}:${issue.message}:${i}`}>
            <div
              role="button"
              tabIndex={0}
              className="validation-issue flex w-full cursor-pointer items-start gap-2 rounded-md p-2 text-left text-xs transition hover:bg-[var(--color-muted)]"
              style={{
                background: `color-mix(in oklch, ${toneColor} 10%, transparent)`,
              }}
              title="クリックして該当するノードまたはパスを選択"
              onClick={() => {
                if (window.getSelection()?.toString()) return;
                onIssueClick?.(issue);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onIssueClick?.(issue);
                }
              }}
            >
              <span className="shrink-0" style={{ color: toneColor }}>
                {icon}
              </span>
              <span className="min-w-0 flex-1">
                {issue.path && (
                  <code className="mr-2 font-mono text-[var(--color-muted-foreground)]">
                    {issue.path}
                  </code>
                )}
                <span className="text-[var(--color-foreground)]">{issue.message}</span>
              </span>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
