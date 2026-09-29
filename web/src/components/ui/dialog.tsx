import type { ReactNode } from 'react';
import { Dialog as Primitive } from '@base-ui/react/dialog';
import { X } from 'lucide-react';
import { Button } from './button';

export function Dialog({ title, label = title, description, onClose, busy = false, children, wide = false }: {
  title: string; label?: string; description?: string; onClose: () => void; busy?: boolean; children: ReactNode; wide?: boolean;
}) {
  return <Primitive.Root open onOpenChange={open => { if (!open && !busy) onClose(); }}>
    <Primitive.Portal><Primitive.Backdrop className="dialog-backdrop" />
      <Primitive.Popup className={`dialog-panel ${wide ? 'dialog-wide' : ''}`} aria-label={label}
        initialFocus={() => document.querySelector<HTMLElement>('[data-autofocus]')}>
        <header className="dialog-header"><div><Primitive.Title>{title}</Primitive.Title>
          {description && <Primitive.Description>{description}</Primitive.Description>}</div>
          <Button variant="quiet" className="icon-button close-btn" aria-label="Close" disabled={busy} onClick={onClose}><X size={17} /></Button>
        </header>{children}
      </Primitive.Popup>
    </Primitive.Portal>
  </Primitive.Root>;
}
export function ErrorMessage({ children }: { children: ReactNode }) {
  return children ? <div className="error-message" role="alert">{children}</div> : null;
}
