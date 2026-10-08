import { Children, Fragment, isValidElement, useEffect, useId, useState, type ComponentPropsWithoutRef, type ReactNode, type Ref } from "react";
import * as Primitive from "@radix-ui/react-select";
import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { useI18n } from "../lib/i18n";

type Option = { value: string; label: ReactNode; text: string; disabled: boolean; group?: string };
type SelectProps = Omit<ComponentPropsWithoutRef<"button">, "value" | "defaultValue" | "onChange" | "children" | "name"> & {
  value?: string | number;
  defaultValue?: string | number;
  onValueChange?: (value: string) => void;
  children: ReactNode;
  name?: string;
  required?: boolean;
  ref?: Ref<HTMLButtonElement>;
  "data-size"?: string;
};

function textOf(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  if (Array.isArray(node)) return node.map(textOf).join("");
  return "";
}

function optionsOf(children: ReactNode, group?: string): Option[] {
  const options: Option[] = [];
  Children.forEach(children, (child) => {
    if (!isValidElement<{ value?: string | number; label?: string; disabled?: boolean; children?: ReactNode }>(child)) return;
    if (child.type === Fragment || child.type === "optgroup") {
      options.push(...optionsOf(child.props.children, child.type === "optgroup" ? child.props.label : group));
    } else if (child.type === "option") {
      const label = child.props.label ?? child.props.children;
      options.push({ value: String(child.props.value ?? textOf(label)), label, text: textOf(label), disabled: Boolean(child.props.disabled), group });
    }
  });
  return options;
}

/** Shared accessible select. Option children describe data, never a second visible control. */
export function Select({ value, defaultValue, onValueChange, children, name, required, disabled, className = "", ref, onKeyDown, onPointerDown, ...props }: SelectProps) {
  const { t } = useI18n();
  const id = useId();
  const emptyValue = `__vf_empty_${id}`;
  const options = optionsOf(children);
  const [localValue, setLocalValue] = useState(String(defaultValue ?? options[0]?.value ?? ""));
  const [open, setOpen] = useState(false);
  const [keyboard, setKeyboard] = useState(false);
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  const currentValue = value === undefined ? localValue : String(value);
  const selected = options.find((option) => option.value === currentValue);
  const encode = (logicalValue: string) => logicalValue === "" ? emptyValue : logicalValue;
  return <Primitive.Root
    value={encode(currentValue)} name={name} required={required} disabled={disabled} open={open} onOpenChange={setOpen}
    onValueChange={(next) => {
      if (disabled) return;
      const logicalValue = next === emptyValue ? "" : next;
      if (value === undefined) setLocalValue(logicalValue);
      onValueChange?.(logicalValue);
    }}
  >
    <Primitive.Trigger {...props} ref={ref} value={currentValue} className={`vf-select-trigger ${className}`}
      onKeyDown={(event) => { onKeyDown?.(event); setKeyboard(true); }}
      onPointerDown={(event) => { onPointerDown?.(event); setKeyboard(false); }}>
      <span className="vf-select-value"><Primitive.Value>{selected?.label ?? t("请选择")}</Primitive.Value></span>
      <Primitive.Icon asChild><ChevronDown size={14} strokeWidth={1.5} aria-hidden="true" /></Primitive.Icon>
    </Primitive.Trigger>
    <Primitive.Portal>
      <Primitive.Content className="vf-select-content" data-keyboard={keyboard || undefined} position="popper" align="end" sideOffset={4} collisionPadding={12}
        onKeyDown={() => setKeyboard(true)} onPointerMove={() => setKeyboard(false)}>
        <Primitive.ScrollUpButton className="vf-select-scroll"><ChevronUp size={14} aria-hidden="true" /></Primitive.ScrollUpButton>
        <Primitive.Viewport className="vf-select-viewport">
          {options.map((option, index) => <Fragment key={option.value}>
            {option.group && option.group !== options[index - 1]?.group && <div className="vf-select-group-label">{option.group}</div>}
            <Primitive.Item value={encode(option.value)} data-value={option.value} disabled={option.disabled} textValue={option.text} className="vf-select-item">
              <Primitive.ItemText>{option.label}</Primitive.ItemText>
              <Primitive.ItemIndicator className="vf-select-check"><Check size={16} strokeWidth={1.7} aria-hidden="true" /></Primitive.ItemIndicator>
            </Primitive.Item>
            {index === 0 && options.length > 1 && ["", "auto", "system", "inherit"].includes(option.value) && <Primitive.Separator className="vf-select-separator" />}
          </Fragment>)}
        </Primitive.Viewport>
        <Primitive.ScrollDownButton className="vf-select-scroll"><ChevronDown size={14} aria-hidden="true" /></Primitive.ScrollDownButton>
      </Primitive.Content>
    </Primitive.Portal>
  </Primitive.Root>;
}
