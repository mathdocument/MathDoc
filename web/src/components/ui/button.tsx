import type { ButtonHTMLAttributes, ReactNode } from "react";
import { Tooltip } from "@base-ui/react/tooltip";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "quiet" | "danger";
};
export function Button({
  variant = "secondary",
  className = "",
  type = "button",
  ...props
}: Props) {
  return (
    <button
      type={type}
      className={`button button-${variant} ${className}`}
      {...props}
    />
  );
}
export function IconButton({
  label,
  children,
  ...props
}: Omit<Props, "children"> & { label: string; children: ReactNode }) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger
        render={
          <Button
            {...props}
            className={`icon-button ${props.className ?? ""}`}
            aria-label={label}
            title={label}
          />
        }
      >
        {children}
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Positioner sideOffset={8}>
          <Tooltip.Popup className="tooltip">{label}</Tooltip.Popup>
        </Tooltip.Positioner>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
