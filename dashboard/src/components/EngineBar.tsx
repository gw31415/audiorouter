import type { EngineConfig } from "../types";

interface Props {
  engine: EngineConfig;
  onChange: (engine: EngineConfig) => void;
  readOnly?: boolean;
}

const SAMPLE_RATES = [44100, 48000, 88200, 96000, 176400, 192000];

/** Compact inline engine controls for the top bar (shadcn style). */
export function EngineBar({ engine, onChange, readOnly = false }: Props) {
  return (
    <div className="flex items-center gap-2 border-l border-[var(--color-border)] pl-4">
      <select
        value={engine.sample_rate}
        disabled={readOnly}
        onChange={(e) => onChange({ ...engine, sample_rate: Number(e.target.value) })}
        className="h-7 rounded-md border border-[var(--color-input)] bg-[var(--color-background)] px-2 text-xs text-[var(--color-foreground)] outline-none transition focus:border-[var(--color-ring)] disabled:opacity-50 disabled:cursor-not-allowed"
        title="Sample Rate"
      >
        {SAMPLE_RATES.map((r) => (
          <option key={r} value={r}>
            {(r / 1000).toFixed(r % 1000 === 0 ? 0 : 1)}kHz
          </option>
        ))}
      </select>
      <input
        type="number"
        value={engine.buffer_size}
        min={16}
        max={8192}
        step={16}
        disabled={readOnly}
        onChange={(e) => onChange({ ...engine, buffer_size: Number(e.target.value) })}
        className="h-7 w-16 rounded-md border border-[var(--color-input)] bg-[var(--color-background)] px-2 text-xs text-[var(--color-foreground)] outline-none transition focus:border-[var(--color-ring)] disabled:opacity-50 disabled:cursor-not-allowed"
        title="Buffer Size"
      />
      <span className="text-[10px] text-[var(--color-muted-foreground)]">buf</span>
    </div>
  );
}
