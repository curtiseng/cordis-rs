export function load() {
  host.log("cite 装上了");
  host.registerTool(
    "cite",
    "按关键词在当前 Markdown 里找出最相关的两三段原文",
    (args) => {
      const md = host.capability("markdown");
      let query = "";
      try {
        const parsed = JSON.parse(args);
        query = String(parsed.input || parsed.query || "");
      } catch {
        query = String(args || "");
      }
      query = query.trim();
      const paras = md
        .split(/\n\s*\n/)
        .map((p) => p.trim())
        .filter(Boolean);
      if (!query) {
        return paras.slice(0, 2).join("\n\n");
      }
      const needle = query.toLowerCase();
      const hit = paras.filter((p) => p.toLowerCase().includes(needle)).slice(0, 3);
      return hit.length ? hit.join("\n\n") : "没有找到含该关键词的段落。";
    },
  );
}

export function unload() {
  host.log("cite 拆掉了");
}
