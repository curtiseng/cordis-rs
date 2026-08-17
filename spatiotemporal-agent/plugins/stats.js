export function load() {
  host.log("stats 装上了");
  host.registerTool(
    "stats",
    "统计当前 Markdown：字数、字符数、段落数、标题数",
    () => {
      const md = host.capability("markdown");
      const text = String(md || "");
      const words = text
        .trim()
        .split(/\s+/)
        .filter(Boolean).length;
      const headings = (text.match(/^#{1,6}\s/gm) || []).length;
      const paragraphs = text
        .split(/\n\s*\n/)
        .map((p) => p.trim())
        .filter(Boolean).length;
      return JSON.stringify({
        chars: text.length,
        words,
        headings,
        paragraphs,
      });
    },
  );
}

export function unload() {
  host.log("stats 拆掉了");
}
