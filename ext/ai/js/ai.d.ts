import { Session } from "./ai.ts";
import { LLMSessionRunInputOptions } from "./llm/llm_session.ts";
import {
  OllamaProviderInput,
  OllamaProviderOptions,
} from "./llm/providers/ollama.ts";
import {
  OpenAIProviderInput,
  OpenAIProviderOptions,
} from "./llm/providers/openai.ts";

export namespace ai {
  export { Session };
  export {
    LLMSessionRunInputOptions as LLMRunOptions,
    OllamaProviderInput as OllamaInput,
    OllamaProviderOptions as OllamaOptions,
    OpenAIProviderInput as OpenAICompatibleInput,
    OpenAIProviderOptions as OpenAICompatibleOptions,
  };
}
