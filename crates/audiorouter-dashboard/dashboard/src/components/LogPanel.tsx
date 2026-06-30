import { useEffect, useRef } from "react";

export interface LogLine {
  level: string;
  message: string;
  timestamp: string;
}

const levelColor = (level: string): string => {
  switch (level.toLowerCase()) {
    case "error":
      return "var(--color-destructive)";
    case "warn":
    case "warning":
      return "var(--color-ar-gain)";
    case "info":
      return "var(--color-ar-in)";
    case "debug":
      return "var(--color-muted-foreground)";
    default:
      return "var(--color-muted-foreground)";
  }
};

export function LogPanel({ lines }: { lines: LogLine[] }) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "instant" });
  }, [lines]);

  if (lines.length === 0) {
    return <p className="text-xs text-[var(--color-muted-foreground)]">ログがありません</p>;
  }

  return (
    <div className="font-mono text-xs leading-relaxed">
      {lines.map((line, i) => (
        <div key={i} className="flex gap-2">
          <span className="shrink-0 text-[var(--color-muted-foreground)]">{line.timestamp}</span>
          <span className="shrink-0 w-10" style={{ color: levelColor(line.level) }}>
            {line.level.toUpperCase().slice(0, 4)}
          </span>
          <span className="break-all" style={{ color: levelColor(line.level) }}>
            {line.message}
          </span>
        </div>
      ))}
      <div ref={bottomRef} />
    </div>
  );
}
