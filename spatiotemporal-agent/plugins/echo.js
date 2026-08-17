export function load() {
  host.log("echo llm 装上了");
  host.registerLlm("echo", (body) => {
    let prompt = "";
    try {
      const req = JSON.parse(body);
      const messages = req.messages || [];
      const last = messages[messages.length - 1];
      prompt = last && last.content ? String(last.content) : "";
    } catch {
      prompt = String(body || "");
    }
    return JSON.stringify({
      choices: [
        {
          message: {
            role: "assistant",
            content: "echo: " + prompt,
          },
        },
      ],
    });
  });
}

export function unload() {
  host.log("echo llm 拆掉了");
}
