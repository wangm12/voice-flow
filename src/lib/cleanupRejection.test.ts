import { describe, expect, it } from "vitest";
import {
  cleanupCandidateLabel,
  cleanupGuardReasonLabel,
  cleanupRejectionStage,
} from "./cleanupRejection";

describe("cleanup rejection contract", () => {
  it("maps the Rust wire reason and stored stage-prefixed reason consistently", () => {
    expect(cleanupGuardReasonLabel("explicit_layout_changed")).toBe("列表或结构未通过保真检查");
    expect(cleanupGuardReasonLabel("provider_protected_token_changed")).toBe("名称、数字或事实未通过保真检查");
    expect(cleanupGuardReasonLabel("final_source_script_changed")).toBe("语言或文字体系未通过保真检查");
  });

  it("keeps provider and final-output candidate stages distinct", () => {
    expect(cleanupRejectionStage("provider_output")).toBe("provider_output");
    expect(cleanupRejectionStage("final_explicit_layout_changed")).toBe("final_output");
    expect(cleanupCandidateLabel("provider_output")).toBe("服务商候选（保真检查未采纳）");
    expect(cleanupCandidateLabel("final_output")).toBe("最终整理候选（保真检查未采纳）");
    expect(cleanupGuardReasonLabel("future_reason")).toBe("具体原因未确认");
  });
});
