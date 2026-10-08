import { useEffect, useId, useRef, useState, type ComponentPropsWithoutRef, type Ref } from "react";
import * as Popover from "@radix-ui/react-popover";
import { Check, ChevronDown } from "lucide-react";
import { useI18n } from "../lib/i18n";

type Suggestion = { value: string; label: string; disabled?: boolean };
type Props = Omit<ComponentPropsWithoutRef<"input">, "value" | "defaultValue" | "onChange" | "list"> & {
  value: string;
  onValueChange: (value: string) => void;
  options: readonly Suggestion[];
  ref?: Ref<HTMLInputElement>;
};

/** Editable suggestions share the dropdown surface while preserving arbitrary input. */
export function Autocomplete({ value, onValueChange, options, disabled, className = "", ref, onFocus, onBlur, onKeyDown, ...props }: Props) {
  const { t } = useI18n();
  const input = useRef<HTMLInputElement>(null);
  const id = useId();
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState(true);
  const [highlighted, setHighlighted] = useState(-1);
  const enabled = options.filter((option) => !option.disabled);
  const needle = value.trim().toLocaleLowerCase();
  const visible = filter && needle ? enabled.filter((option) => `${option.value} ${option.label}`.toLocaleLowerCase().includes(needle)) : enabled;
  const expanded = open && !disabled && visible.length > 0;
  const activeId = expanded && highlighted >= 0 && highlighted < visible.length ? `${id}-${highlighted}` : undefined;
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  useEffect(() => { if (activeId) document.getElementById(activeId)?.scrollIntoView({ block: "nearest" }); }, [activeId]);
  const choose = (next: string) => {
    if (disabled) return;
    onValueChange(next);
    setOpen(false);
    setHighlighted(-1);
    input.current?.focus({ preventScroll: true });
  };
  return <Popover.Root open={expanded} onOpenChange={setOpen} modal={false}>
    <Popover.Anchor asChild>
      <div className={`vf-autocomplete ${className}`} data-disabled={disabled || undefined} data-invalid={props["aria-invalid"] || undefined}>
        <input {...props} value={value} disabled={disabled} className="vf-autocomplete-input"
          ref={(node) => { input.current = node; if (typeof ref === "function") ref(node); else if (ref) ref.current = node; }}
          role={options.length ? "combobox" : undefined} aria-autocomplete={options.length ? "list" : undefined}
          aria-haspopup={options.length ? "listbox" : undefined} aria-expanded={options.length ? expanded : undefined}
          aria-controls={expanded ? id : undefined} aria-activedescendant={activeId}
          onChange={(event) => { onValueChange(event.target.value); setFilter(true); setHighlighted(-1); setOpen(true); }}
          onFocus={(event) => { onFocus?.(event); setFilter(true); setOpen(true); }}
          onBlur={(event) => { onBlur?.(event); setOpen(false); }}
          onKeyDown={(event) => {
            onKeyDown?.(event);
            if (event.defaultPrevented || !enabled.length) return;
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              if (!expanded) { setFilter(false); setOpen(true); setHighlighted(event.key === "ArrowDown" ? 0 : enabled.length - 1); }
              else setHighlighted((index) => event.key === "ArrowDown" ? (index + 1) % visible.length : (index - 1 + visible.length) % visible.length);
            } else if (expanded && (event.key === "Home" || event.key === "End")) {
              event.preventDefault(); setHighlighted(event.key === "Home" ? 0 : visible.length - 1);
            } else if (expanded && event.key === "Enter" && highlighted >= 0 && visible[highlighted]) {
              event.preventDefault(); choose(visible[highlighted].value);
            } else if (expanded && event.key === "Escape") {
              event.preventDefault(); event.stopPropagation(); setOpen(false); setHighlighted(-1);
            } else if (event.key === "Tab") setOpen(false);
          }}
        />
        {options.length > 0 && <button type="button" className="vf-autocomplete-toggle" tabIndex={-1} disabled={disabled} aria-label={t("显示建议")}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => { input.current?.focus(); setFilter(false); setHighlighted(-1); setOpen(!expanded); }}><ChevronDown size={14} strokeWidth={1.5} aria-hidden="true" /></button>}
      </div>
    </Popover.Anchor>
    <Popover.Portal>
      <Popover.Content role="listbox" id={id} aria-label={props["aria-label"]} className="vf-select-content vf-autocomplete-content" align="end" sideOffset={4} collisionPadding={12}
        onOpenAutoFocus={(event) => event.preventDefault()} onCloseAutoFocus={(event) => event.preventDefault()}
        onInteractOutside={(event) => { if (event.target === input.current) event.preventDefault(); }}>
        <div>
          {visible.map((option, index) => <div key={option.value} id={`${id}-${index}`} role="option" aria-selected={option.value === value}
            data-value={option.value} data-state={option.value === value ? "checked" : "unchecked"} data-highlighted={highlighted === index ? "" : undefined}
            className="vf-select-item" onMouseDown={(event) => event.preventDefault()} onMouseMove={() => setHighlighted(index)} onClick={() => choose(option.value)}>
            <span>{option.label}</span>{option.value === value && <Check size={16} strokeWidth={1.7} aria-hidden="true" />}
          </div>)}
        </div>
      </Popover.Content>
    </Popover.Portal>
  </Popover.Root>;
}
