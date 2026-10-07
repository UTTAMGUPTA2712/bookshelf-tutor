import { useEffect, useRef } from "react";
import { Spinner } from "../ui";

/** Live view of what the model is writing, so slow local models don't look frozen. */
export function StreamBox({ status, text }: { status: string; text: string }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [text]);
  return (
    <div>
      <div className="mb-2 flex items-center gap-2 text-sm">
        <Spinner />
        <span>{status || "Working…"}</span>
        {text && <span className="ml-auto text-xs text-muted">{text.length.toLocaleString()} characters</span>}
      </div>
      {text && (
        <pre
          ref={ref}
          className="max-h-40 overflow-y-auto rounded-md bg-sunken p-2 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-muted"
        >
          {text.slice(-4000)}
        </pre>
      )}
    </div>
  );
}
