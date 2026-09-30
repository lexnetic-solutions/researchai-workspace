import type { ReactNode } from 'react';

interface EmptyStateProps {
  readonly title: string;
  readonly hint: string;
  readonly action?: ReactNode;
}

export function EmptyState({ title, hint, action }: EmptyStateProps) {
  return (
    <div className="empty-state">
      <div className="empty-state-art" aria-hidden="true">
        ▤
      </div>
      <h3>{title}</h3>
      <p>{hint}</p>
      {action && <div className="empty-state-action">{action}</div>}
    </div>
  );
}
