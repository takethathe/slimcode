// The Card view's renderer (ADR-0030 D6/D8): the ten catalog components in the
// GUI's slate/indigo language, plus the quiet placeholders for unknown types
// and for a block that throws while rendering.
//
// Every entry is `guarded`: the real body renders as a child of a local error
// boundary, so one malformed element degrades to a stub instead of taking the
// rest of the Card (or the panel) down with it. json-render's own boundary
// renders nothing on error, which would leave a hole.

import { Component, type ReactNode } from "react";
import { JSONUIProvider, Renderer, defineRegistry } from "@json-render/react";
import type { Spec } from "@json-render/core";

import { Markdown } from "./components/Markdown";
import { catalog } from "./cardCatalog";

class BlockBoundary extends Component<
  { label: string; children: ReactNode },
  { failed: boolean }
> {
  constructor(props: { label: string; children: ReactNode }) {
    super(props);
    this.state = { failed: false };
  }

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  render(): ReactNode {
    if (this.state.failed) return <Stub label={this.props.label} />;
    return this.props.children;
  }
}

function Stub({ label }: { label: string }) {
  return (
    <div className="rounded-md border border-dashed border-slate-200 px-2 py-1 text-xs text-slate-400">
      {label} unavailable
    </div>
  );
}

const GAP = { sm: "gap-1", md: "gap-2", lg: "gap-3" } as const;
const SIZE = { sm: "text-xs", md: "text-sm", lg: "text-base" } as const;
const WEIGHT = {
  normal: "text-slate-700",
  muted: "text-slate-500",
  strong: "font-medium text-slate-900",
} as const;
const TONE = {
  neutral: "bg-slate-100 text-slate-600",
  info: "bg-indigo-50 text-indigo-700",
  success: "bg-emerald-50 text-emerald-700",
  warning: "bg-amber-50 text-amber-700",
  danger: "bg-red-50 text-red-700",
} as const;

const { registry } = defineRegistry(catalog, {
  components: {
    Card: ({ props, children }) => (
      <BlockBoundary label="Card">
        <section className="rounded-lg border border-slate-200 bg-white px-3 py-2.5 shadow-sm">
          <header className="mb-2 border-b border-slate-100 pb-1.5">
            <h3 className="text-sm font-semibold text-slate-900">{props.title}</h3>
            {props.subtitle ? (
              <p className="text-xs text-slate-500">{props.subtitle}</p>
            ) : null}
          </header>
          <div className="flex flex-col gap-2">{children}</div>
        </section>
      </BlockBoundary>
    ),
    Stack: ({ props, children }) => (
      <BlockBoundary label="Stack">
        <div
          className={`flex ${props.direction === "horizontal" ? "flex-row flex-wrap items-center" : "flex-col"} ${GAP[props.gap ?? "md"]}`}
        >
          {children}
        </div>
      </BlockBoundary>
    ),
    Text: ({ props }) => (
      <BlockBoundary label="Text">
        <p
          className={`break-words leading-relaxed ${SIZE[props.size ?? "md"]} ${WEIGHT[props.weight ?? "normal"]}`}
        >
          {props.content}
        </p>
      </BlockBoundary>
    ),
    List: ({ props }) => (
      <BlockBoundary label="List">
        <ul className="flex flex-col gap-1 text-sm text-slate-700">
          {props.items.map((item, index) => (
            <li key={index} className="flex items-start gap-1.5">
              <span
                aria-hidden
                className={`mt-0.5 size-3.5 shrink-0 rounded-full border text-center text-[0.6rem] leading-3 ${
                  item.done
                    ? "border-emerald-300 bg-emerald-50 text-emerald-600"
                    : "border-slate-300 text-transparent"
                }`}
              >
                ✓
              </span>
              <span className={item.done ? "text-slate-400 line-through" : ""}>
                {item.text}
              </span>
            </li>
          ))}
        </ul>
      </BlockBoundary>
    ),
    KeyValue: ({ props }) => (
      <BlockBoundary label="KeyValue">
        <div className="flex items-baseline justify-between gap-3 text-sm">
          <span className="text-slate-500">{props.label}</span>
          <span className="min-w-0 break-words text-right font-mono text-xs text-slate-800">
            {props.value}
          </span>
        </div>
      </BlockBoundary>
    ),
    Badge: ({ props }) => (
      <BlockBoundary label="Badge">
        <span
          className={`inline-flex w-fit items-center rounded-full px-2 py-0.5 text-xs font-medium ${TONE[props.tone ?? "neutral"]}`}
        >
          {props.text}
        </span>
      </BlockBoundary>
    ),
    Progress: ({ props }) => {
      const value = Math.max(0, Math.min(100, props.value));
      return (
        <BlockBoundary label="Progress">
          <div className="flex flex-col gap-1">
            {props.label ? (
              <div className="flex justify-between text-xs text-slate-500">
                <span>{props.label}</span>
                <span>{Math.round(value)}%</span>
              </div>
            ) : null}
            <div
              role="progressbar"
              aria-valuenow={Math.round(value)}
              aria-valuemin={0}
              aria-valuemax={100}
              className="h-1.5 overflow-hidden rounded-full bg-slate-100"
            >
              <div
                className="h-full rounded-full bg-indigo-500 transition-[width] duration-300"
                style={{ width: `${value}%` }}
              />
            </div>
          </div>
        </BlockBoundary>
      );
    },
    Divider: () => (
      <BlockBoundary label="Divider">
        <hr className="border-slate-100" />
      </BlockBoundary>
    ),
    Markdown: ({ props }) => (
      <BlockBoundary label="Markdown">
        <Markdown text={props.content} />
      </BlockBoundary>
    ),
  },
  // Read-only: the catalog declares no actions to bind.
  actions: {},
});

/** Unknown component types degrade to the same quiet stub as a failed block. */
function UnknownType({ element }: { element: { type: string } }) {
  return <Stub label={element.type} />;
}

/** Render one Card's spec. Each Card owns its own provider so a Card's state
 *  never leaks into a sibling. */
export function CardRenderer({ spec }: { spec: Spec | null }) {
  return (
    <JSONUIProvider registry={registry}>
      <Renderer spec={spec} registry={registry} fallback={UnknownType} />
    </JSONUIProvider>
  );
}

export { registry as cardRegistry };
