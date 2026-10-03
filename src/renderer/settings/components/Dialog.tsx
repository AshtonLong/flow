import type { ReactNode } from 'react';
import { AlertDialog, Dialog } from 'radix-ui';
import { X } from 'lucide-react';
import { Button, IconButton } from './Button';

/** A yes/no question before something is removed. Esc and Cancel both back out. */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  /** Names the action: "Remove model", not "OK". */
  confirmLabel: string;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog.Root open={open} onOpenChange={onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-smoke" />
        <AlertDialog.Content className="dialog">
          <div className="px-6 pt-6 pb-5">
            <AlertDialog.Title className="font-display text-subtitle font-[760]">
              {title}
            </AlertDialog.Title>
            <AlertDialog.Description className="mt-2 text-fg-2">
              {description}
            </AlertDialog.Description>
          </div>
          <div className="flex justify-end gap-2 border-t border-line bg-inset px-6 py-4">
            <AlertDialog.Action asChild>
              <Button variant="accent" className="min-w-[120px]" onClick={onConfirm}>
                {confirmLabel}
              </Button>
            </AlertDialog.Action>
            <AlertDialog.Cancel asChild>
              <Button className="min-w-[120px]">Cancel</Button>
            </AlertDialog.Cancel>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

/** A modal editor. Changes inside apply as they are made, so the only action is Done. */
export function Modal({
  open,
  onOpenChange,
  title,
  description,
  width = 560,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  width?: number;
  children: ReactNode;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-smoke" />
        <Dialog.Content
          className="dialog"
          style={{ ['--dialog-width' as string]: `${width}px` }}
          // Focus the dialog itself, not its Close button, so no tooltip pops up on open.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement).focus();
          }}
        >
          <div className="flex items-start justify-between gap-4 px-6 pt-5 pb-3">
            <div>
              <Dialog.Title className="font-display text-subtitle font-[760]">{title}</Dialog.Title>
              <Dialog.Description
                className={description ? 'mt-0.5 text-caption text-fg-2' : 'sr-only'}
              >
                {description ?? title}
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <IconButton label="Close" icon={<X size={16} />} className="-mr-2" />
            </Dialog.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-5">{children}</div>
          <div className="flex justify-end border-t border-line bg-inset px-6 py-4">
            <Dialog.Close asChild>
              <Button variant="accent" className="min-w-[120px]">
                Done
              </Button>
            </Dialog.Close>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
