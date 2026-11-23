import { cli } from "./compare.mjs";

try {
  await cli(process.argv.slice(2));
} catch (error) {
  console.error(error.message);
  process.exitCode = 2;
}
