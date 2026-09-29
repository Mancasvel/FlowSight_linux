// Cargo check runs Tauri's build script, which validates bundle resource paths.
// These empty placeholders are for compile-only CI jobs. Release builds run
// fetch-models.mjs and must bundle the real, size-checked model files.
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const modelDir = join(root, 'local_llm');
mkdirSync(modelDir, { recursive: true });

for (const name of [
  'Qwen3VL-2B-Instruct-Q4_K_M.gguf',
  'mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf',
]) {
  const file = join(modelDir, name);
  if (!existsSync(file)) writeFileSync(file, '');
}
