import { useMemo } from 'react';

/**
 * Renders text with [n] citation markers as clickable chips that scroll to
 * the element with id `target-${n}` (an evidence card, a matrix item, …).
 */
export function CitedText({
  answer,
  targetPrefix,
}: {
  answer: string;
  targetPrefix: string;
}) {
  const parts = useMemo(() => answer.split(/(\[\d+\])/g), [answer]);
  return (
    <div className="answer-text">
      {parts.map((part, i) => {
        const m = part.match(/^\[(\d+)\]$/);
        if (m) {
          const n = Number(m[1]);
          return (
            <button
              key={i}
              type="button"
              className="citation-chip"
              title={`Jump to reference [${n}]`}
              onClick={() =>
                document
                  .getElementById(`${targetPrefix}-${n}`)
                  ?.scrollIntoView({ behavior: 'smooth', block: 'center' })
              }
            >
              [{n}]
            </button>
          );
        }
        return <span key={i}>{part}</span>;
      })}
    </div>
  );
}
