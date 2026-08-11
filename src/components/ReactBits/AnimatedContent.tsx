import type { CSSProperties, ReactNode } from "react";

type AnimatedContentProps = {
  children: ReactNode;
  className?: string;
  delay?: number;
};

/** A small, dependency-free reveal inspired by React Bits' content transitions. */
export function AnimatedContent({ children, className = "", delay = 0 }: AnimatedContentProps) {
  const style = { "--rb-delay": `${delay}ms` } as CSSProperties;
  return <div className={`rb-animated-content ${className}`.trim()} style={style}>{children}</div>;
}
