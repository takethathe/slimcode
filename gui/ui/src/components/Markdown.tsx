// The assistant-prose seam: markdown in, readable markup out (spec
// .scratch/gui-markdown/spec.md). Every display rule for assistant text lives
// here — heading markers, code blocks, tables, links — so the transcript's
// block model keeps holding raw text and live streaming and session replay
// render through this one function.
//
// `skipHtml` drops bare HTML: the model occasionally emits tags, and the GUI
// never trusts model output as markup.

import { Children, isValidElement, type ReactNode } from "react";
import ReactMarkdown, { type Options } from "react-markdown";
import remarkGfm from "remark-gfm";

import { isExternalUrl, openExternal } from "../external";

// GFM parses two constructs this seam deliberately does not render (spec
// .scratch/gui-markdown: task lists become text, footnotes stay off). Left
// alone they break the transcript: a checkbox is a form control sitting in
// prose, and a footnote draws an auto-generated "Footnotes" heading — a
// heading the model never wrote, which corrupts the `#`-count hierarchy — plus
// anchors the link handler can never open. Both are degraded at the mdast
// level, so the component mappings below only ever see plain text and
// paragraphs.
type MdNode = {
  type: string;
  value?: string;
  identifier?: string;
  checked?: boolean | null;
  children?: MdNode[];
};

function degradeGfmExtras(node: MdNode): void {
  if (!node.children) return;
  const kept: MdNode[] = [];
  for (const child of node.children) {
    if (child.type === "footnoteDefinition") continue;
    if (child.type === "footnoteReference") {
      kept.push({ type: "text", value: `[^${child.identifier ?? ""}]` });
      continue;
    }
    if (child.type === "listItem" && typeof child.checked === "boolean") {
      const marker = child.checked ? "☑ " : "☐ ";
      child.checked = null;
      const first = child.children?.[0];
      if (first?.type === "paragraph" && first.children) {
        first.children.unshift({ type: "text", value: marker });
      }
    }
    degradeGfmExtras(child);
    kept.push(child);
  }
  node.children = kept;
}

type RemarkPlugin = NonNullable<Options["remarkPlugins"]>[number];

const remarkDegradeGfmExtras: RemarkPlugin = function () {
  return function (tree) {
    degradeGfmExtras(tree as MdNode);
  };
};

// A heading renders as a real `h1`–`h6` and keeps its source `#`×N in front as a
// dim monospace prefix. The marker count is the hierarchy's primary signal; the
// font size is decoration in two tiers only (h1/h2 a step above body text, h3
// and deeper at body size), because a size difference alone cannot tell a
// level-2 heading from a level-3 one (spec: Heading marker, ADR-0028).
type HeadingLevel = 1 | 2 | 3 | 4 | 5 | 6;

function Heading({
  level,
  children,
}: {
  level: HeadingLevel;
  children: ReactNode;
}) {
  const Tag = `h${level}` as const;
  const size = level <= 2 ? "text-[1.125rem]" : "text-sm";
  return (
    <Tag
      className={`mt-4 mb-2 font-semibold text-slate-900 first:mt-0 ${size}`}
    >
      <span className="font-mono font-normal text-slate-400">
        {`${"#".repeat(level)} `}
      </span>
      {children}
    </Tag>
  );
}

// A fenced block's info string (```rust) arrives as the `language-*` class on
// the `<code>` child of `<pre>`, so the block wrapper reads it from there: it
// owns the grey block, the one-line scroll container and the corner language
// label. Inline code is styled from `index.css` (`.markdown :not(pre) > code`)
// so the two forms never share a selector.
function languageOf(children: ReactNode): string | undefined {
  const first = Children.toArray(children)[0];
  if (!isValidElement<{ className?: string }>(first)) return undefined;
  return /language-(\S+)/.exec(first.props.className ?? "")?.[1];
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const language = languageOf(children);
  return (
    <div className="my-3 rounded-lg border border-slate-200 bg-slate-100">
      {language ? (
        <div className="flex justify-end px-3 pt-1.5 font-mono text-[0.65rem] uppercase tracking-wide text-slate-400">
          {language}
        </div>
      ) : null}
      <pre
        data-language={language}
        className="overflow-x-auto px-3 py-2 font-mono text-xs leading-relaxed text-slate-800"
      >
        {children}
      </pre>
    </div>
  );
}

export function Markdown({ text }: { text: string }) {
  return (
    <div className="markdown min-w-0 break-words font-sans text-sm leading-relaxed text-slate-800">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkDegradeGfmExtras]}
        skipHtml
        components={{
          h1: ({ children }) => <Heading level={1}>{children}</Heading>,
          h2: ({ children }) => <Heading level={2}>{children}</Heading>,
          h3: ({ children }) => <Heading level={3}>{children}</Heading>,
          h4: ({ children }) => <Heading level={4}>{children}</Heading>,
          h5: ({ children }) => <Heading level={5}>{children}</Heading>,
          h6: ({ children }) => <Heading level={6}>{children}</Heading>,
          p: ({ children }) => <p className="my-2 first:mt-0 last:mb-0">{children}</p>,
          strong: ({ children }) => (
            <strong className="font-semibold text-slate-900">{children}</strong>
          ),
          em: ({ children }) => <em className="italic">{children}</em>,
          ul: ({ children }) => (
            <ul className="my-2 list-disc space-y-1 pl-5 marker:text-slate-400">
              {children}
            </ul>
          ),
          ol: ({ children }) => (
            <ol className="my-2 list-decimal space-y-1 pl-5 marker:text-slate-400">
              {children}
            </ol>
          ),
          blockquote: ({ children }) => (
            <blockquote className="my-2 border-l-2 border-slate-300 pl-3 text-slate-600">
              {children}
            </blockquote>
          ),
          // A wide table scrolls inside its own container: the transcript never
          // grows a horizontal scrollbar because a model wrote a 9-column table.
          table: ({ children }) => (
            <div className="my-3 overflow-x-auto">
              <table className="w-full border-collapse text-left text-[0.8125rem]">
                {children}
              </table>
            </div>
          ),
          th: ({ children }) => (
            <th className="border border-slate-200 bg-slate-50 px-2 py-1 font-semibold text-slate-700">
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td className="border border-slate-200 px-2 py-1 align-top">
              {children}
            </td>
          ),
          pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
          // Image rendering is out of scope, and the webview's CSP is null: a
          // model-emitted `![](https://…)` would become a remote request from
          // the transcript (a tracking pixel). Render the alt text instead.
          img: ({ alt }) => (
            <span className="italic text-slate-500">[{alt || "image"}]</span>
          ),
          // The webview must never navigate away from the conversation, so every
          // pointer path that can activate a link is cancelled: `click` (left /
          // ctrl-cmd) and `auxclick` (middle-click / "open in new tab"), which
          // fires instead of `click` and would otherwise slip past. Only an
          // `http(s)` target is handed to the system browser (../external); the
          // host's `on_navigation` allowlist is the runtime backstop (ADR-0028).
          a: ({ href, children }) => (
            <a
              href={href}
              className="text-indigo-600 underline decoration-indigo-300 underline-offset-2 hover:text-indigo-700"
              onClick={(event) => {
                event.preventDefault();
                if (isExternalUrl(href)) void openExternal(href);
              }}
              onAuxClick={(event) => event.preventDefault()}
            >
              {children}
            </a>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
