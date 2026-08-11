import type { InputHTMLAttributes } from "react";

type CurvedInputProps = InputHTMLAttributes<HTMLInputElement> & {
  wrapperClassName?: string;
};

/** Keeps the React Bits curved-focus treatment while preserving a native input. */
export function CurvedInput({ className = "", wrapperClassName = "", ...props }: CurvedInputProps) {
  return (
    <div className={`rb-curved-input ${wrapperClassName}`.trim()}>
      <input {...props} className={`rb-curved-input__field ${className}`.trim()} />
    </div>
  );
}
