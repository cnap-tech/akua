/* @ts-self-types="./akua_wasm.d.ts" */
import * as wasm from "./akua_wasm_bg.wasm";
import { __wbg_set_wasm } from "./akua_wasm_bg.js";

__wbg_set_wasm(wasm);
wasm.__wbindgen_start();
export {
    applyInputTransforms, buildMetadata, buildUmbrellaChart, extractUserInputFields, init, mergeSourceValues, mergeValuesSchemas, validateValuesSchema
} from "./akua_wasm_bg.js";
