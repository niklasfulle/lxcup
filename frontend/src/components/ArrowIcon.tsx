type ArrowIconProps = Readonly<{
  className?: string;
  direction?: "left" | "right";
}>;

export function ArrowIcon({ className = "h-5 w-5", direction = "right" }: ArrowIconProps) {
  const path = direction === "left"
    ? "M16.5 10h-12m5 5-5-5 5-5"
    : "M3.5 10h12m-5-5 5 5-5 5";

  return <svg className={`shrink-0 ${className}`} viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={path} /></svg>;
}
