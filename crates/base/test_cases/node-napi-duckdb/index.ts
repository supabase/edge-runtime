import { DuckDBInstance } from "npm:@duckdb/node-api";

Deno.serve(async (_req: Request) => {
  const instance = await DuckDBInstance.create(":memory:");
  const connection = await instance.connect();

  try {
    const reader = await connection.runAndReadAll(
      "SELECT 42 AS num, 'v' || version() AS version",
    );
    const rows = reader.getRows();

    return Response.json({ rows, success: true });
  } finally {
    connection.closeSync();
    instance.closeSync();
  }
});
