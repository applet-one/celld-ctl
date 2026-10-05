export class AppletState {
  constructor(ctx) {
    this.ctx = ctx;
  }

  async fetch() {
    this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS counters (value INTEGER NOT NULL)");
    this.ctx.storage.sql.exec("INSERT INTO counters (value) SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM counters)");
    const row = this.ctx.storage.sql.exec("UPDATE counters SET value = value + 1 RETURNING value").one();
    return Response.json({ count: row.value });
  }
}

export default {
  async fetch(request, env) {
    const id = env.APPLET_STATE.idFromName("default");
    return env.APPLET_STATE.get(id).fetch(request);
  },
};
