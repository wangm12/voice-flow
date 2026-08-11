import type { LucideProps } from "lucide-react";

export const iconProps = {
  size: 18,
  strokeWidth: 1.5,
  absoluteStrokeWidth: true,
} as const satisfies Partial<LucideProps>;

export const iconPropsSm = {
  size: 16,
  strokeWidth: 1.5,
  absoluteStrokeWidth: true,
} as const satisfies Partial<LucideProps>;

export const iconPropsLg = {
  size: 20,
  strokeWidth: 1.5,
  absoluteStrokeWidth: true,
} as const satisfies Partial<LucideProps>;
