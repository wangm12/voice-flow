/*
 * @license MIT, Copyright (c) 2026 Kubai087
 * Adapted from Glasswork/refract.js at 4f0b6744b898b3a9b98fbd913ab24fdb83c53162.
 * Full MIT text and source pin: THIRD_PARTY_NOTICES.md (Glasswork).
 */

const VERTEX_SHADER = `#version 300 es
in vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }`;

// One-surface adaptation of Glasswork's rounded-box refraction shader.
const FRAGMENT_SHADER = `#version 300 es
precision highp float;
uniform sampler2D uTex;
uniform vec2 uRes;
uniform float uScale;
uniform vec2 uOffset;
uniform vec2 uSceneSize;
uniform vec4 uShape;
uniform vec4 uTone;
uniform vec2 uMore;
out vec4 outColor;

float box(vec2 p, vec2 halfSize, float radius) {
  vec2 q = abs(p) - halfSize + radius;
  return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - radius;
}

// The 2D source canvas and this material share CSS-pixel coordinates.
vec3 scene(vec2 p) {
  vec2 uv = clamp((p + uOffset) / uSceneSize, 0.0, 1.0);
  return texture(uTex, uv).rgb;
}

vec3 blurred(vec2 p, float radius) {
  if (radius < 0.6) return scene(p);
  vec3 sum = scene(p);
  float total = 1.0;
  for (int i = 0; i < 12; i++) {
    float fi = float(i);
    float sampleDistance = radius * 2.2 * sqrt((fi + 0.5) / 12.0);
    float angle = fi * 2.39996;
    float weight = exp(-(sampleDistance * sampleDistance) / (2.0 * radius * radius));
    sum += scene(p + vec2(cos(angle), sin(angle)) * sampleDistance) * weight;
    total += weight;
  }
  return sum / total;
}

vec3 saturateBy(vec3 c, float amount) {
  return vec3(
    (0.213 + 0.787 * amount) * c.r + (0.715 - 0.715 * amount) * c.g + (0.072 - 0.072 * amount) * c.b,
    (0.213 - 0.213 * amount) * c.r + (0.715 + 0.285 * amount) * c.g + (0.072 - 0.072 * amount) * c.b,
    (0.213 - 0.213 * amount) * c.r + (0.715 - 0.715 * amount) * c.g + (0.072 + 0.928 * amount) * c.b);
}

void main() {
  vec2 p = vec2(gl_FragCoord.x, uRes.y - gl_FragCoord.y) / uScale;
  vec2 halfSize = (uRes / uScale) * 0.5;
  float radius = min(uShape.x, min(halfSize.x, halfSize.y));
  float d = box(p - halfSize, halfSize, radius);
  if (d > 0.75) {
    outColor = vec4(0.0);
    return;
  }

  float epsilon = 0.5;
  vec2 gradient = vec2(
    box(p - halfSize + vec2(epsilon, 0.0), halfSize, radius)
      - box(p - halfSize - vec2(epsilon, 0.0), halfSize, radius),
    box(p - halfSize + vec2(0.0, epsilon), halfSize, radius)
      - box(p - halfSize - vec2(0.0, epsilon), halfSize, radius));
  vec2 inward = -gradient / max(length(gradient), 1e-5);
  float rim = uShape.y;
  float depth = clamp(-d / rim, 0.0, 1.0);
  float amount = clamp(uShape.z * rim * pow(1.0 - depth, uShape.w), 0.0, rim * 2.5);
  float split = uMore.y;
  vec3 color;
  if (split > 0.0 && amount > 0.01) {
    color.r = blurred(p + inward * amount * (1.0 + split), uTone.x).r;
    color.g = blurred(p + inward * amount, uTone.x).g;
    color.b = blurred(p + inward * amount * (1.0 - split), uTone.x).b;
  } else {
    color = blurred(p + inward * amount, uTone.x);
  }
  color = saturateBy(color, uTone.y);
  color = (color - 0.5) * uTone.z + 0.5;
  color *= uTone.w;
  color = saturateBy(color, uMore.x);
  float alpha = clamp(0.5 - d * uScale, 0.0, 1.0);
  outColor = vec4(clamp(color, 0.0, 1.0) * alpha, alpha);
}`;

export type GlassworkRefraction = {
  render(options: {
    source: HTMLCanvasElement;
    scale: number;
    offsetX: number;
    offsetY: number;
    sceneWidth: number;
    sceneHeight: number;
    width: number;
    height: number;
  }): boolean;
  dispose(): void;
};

function compileShader(gl: WebGL2RenderingContext, type: number, source: string) {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("glasswork_shader_create_failed");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader) ?? "glasswork_shader_compile_failed";
    gl.deleteShader(shader);
    throw new Error(log);
  }
  return shader;
}

export function createGlassworkRefraction(canvas: HTMLCanvasElement): GlassworkRefraction | null {
  const gl = canvas.getContext("webgl2", {
    alpha: true,
    premultipliedAlpha: true,
    antialias: false,
    preserveDrawingBuffer: true,
  });
  if (!gl) return null;

  let vertex: WebGLShader | null = null;
  let fragment: WebGLShader | null = null;
  let program: WebGLProgram | null = null;
  let buffer: WebGLBuffer | null = null;
  let texture: WebGLTexture | null = null;
  try {
    vertex = compileShader(gl, gl.VERTEX_SHADER, VERTEX_SHADER);
    fragment = compileShader(gl, gl.FRAGMENT_SHADER, FRAGMENT_SHADER);
    program = gl.createProgram();
    if (!program) throw new Error("glasswork_program_create_failed");
    gl.attachShader(program, vertex);
    gl.attachShader(program, fragment);
    gl.bindAttribLocation(program, 0, "aPos");
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      throw new Error(gl.getProgramInfoLog(program) ?? "glasswork_program_link_failed");
    }

    buffer = gl.createBuffer();
    texture = gl.createTexture();
    if (!buffer || !texture) throw new Error("glasswork_resource_create_failed");
    gl.useProgram(program);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

    const uniforms = {
      resolution: gl.getUniformLocation(program, "uRes"),
      scale: gl.getUniformLocation(program, "uScale"),
      offset: gl.getUniformLocation(program, "uOffset"),
      sceneSize: gl.getUniformLocation(program, "uSceneSize"),
      shape: gl.getUniformLocation(program, "uShape"),
      tone: gl.getUniformLocation(program, "uTone"),
      more: gl.getUniformLocation(program, "uMore"),
      texture: gl.getUniformLocation(program, "uTex"),
    };
    if (Object.values(uniforms).some((uniform) => uniform === null)) {
      throw new Error("glasswork_uniform_lookup_failed");
    }
    gl.uniform1i(uniforms.texture, 0);

    gl.deleteShader(vertex);
    gl.deleteShader(fragment);
    vertex = null;
    fragment = null;

    let disposed = false;
    return {
      render({ source, scale, offsetX, offsetY, sceneWidth, sceneHeight, width, height }) {
        if (disposed || gl.isContextLost() || !source.width || !source.height || !width || !height) return false;
        try {
          gl.useProgram(program);
          gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
          gl.bindTexture(gl.TEXTURE_2D, texture);
          gl.activeTexture(gl.TEXTURE0);
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source);
          gl.viewport(0, 0, canvas.width, canvas.height);
          gl.clearColor(0, 0, 0, 0);
          gl.clear(gl.COLOR_BUFFER_BIT);
          gl.enable(gl.BLEND);
          gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
          gl.uniform2f(uniforms.resolution, canvas.width, canvas.height);
          gl.uniform1f(uniforms.scale, scale);
          gl.uniform2f(uniforms.offset, offsetX, offsetY);
          gl.uniform2f(uniforms.sceneSize, sceneWidth, sceneHeight);
          gl.uniform4f(uniforms.shape, 24, 14, 0.8, 2.6);
          gl.uniform4f(uniforms.tone, 1.25, 1.12, 1.02, 1.02);
          gl.uniform2f(uniforms.more, 1.0, 0.08);
          gl.drawArrays(gl.TRIANGLES, 0, 3);
          return gl.getError() === gl.NO_ERROR;
        } catch {
          return false;
        }
      },
      dispose() {
        if (disposed) return;
        disposed = true;
        if (gl.isContextLost()) return;
        if (program) gl.deleteProgram(program);
        if (buffer) gl.deleteBuffer(buffer);
        if (texture) gl.deleteTexture(texture);
      },
    };
  } catch {
    if (program) gl.deleteProgram(program);
    if (buffer) gl.deleteBuffer(buffer);
    if (texture) gl.deleteTexture(texture);
    if (vertex) gl.deleteShader(vertex);
    if (fragment) gl.deleteShader(fragment);
    return null;
  }
}

export function drawGlassworkScene(
  canvas: HTMLCanvasElement,
  width: number,
  height: number,
  scale: number,
  backdrop: "light" | "dark",
) {
  const context = canvas.getContext("2d");
  if (!context || width <= 0 || height <= 0) return false;
  const pixelWidth = Math.max(1, Math.round(width * scale));
  const pixelHeight = Math.max(1, Math.round(height * scale));
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }
  context.setTransform(scale, 0, 0, scale, 0, 0);
  context.clearRect(0, 0, width, height);

  const dark = backdrop === "dark";
  const background = context.createLinearGradient(0, 0, width, height);
  if (dark) {
    background.addColorStop(0, "#3a3d42");
    background.addColorStop(0.42, "#292c30");
    background.addColorStop(1, "#191b1e");
  } else {
    background.addColorStop(0, "#dde8ef");
    background.addColorStop(0.48, "#edf0f0");
    background.addColorStop(1, "#f2eee8");
  }
  context.fillStyle = background;
  context.fillRect(0, 0, width, height);

  const beam = context.createLinearGradient(0, 0, width * 0.78, height * 0.72);
  beam.addColorStop(0, dark ? "rgba(230, 233, 238, 0.22)" : "rgba(255, 255, 255, 0.78)");
  beam.addColorStop(0.2, dark ? "rgba(187, 192, 199, 0.09)" : "rgba(255, 255, 255, 0.42)");
  beam.addColorStop(0.62, dark ? "rgba(142, 148, 157, 0.025)" : "rgba(212, 228, 238, 0.08)");
  beam.addColorStop(1, "rgba(255, 255, 255, 0)");
  context.fillStyle = beam;
  context.fillRect(0, 0, width, height);

  const bloom = context.createRadialGradient(width * 0.14, height * 0.12, 2, width * 0.14, height * 0.12, Math.max(width, height) * 0.8);
  bloom.addColorStop(0, dark ? "rgba(224, 228, 234, 0.11)" : "rgba(255, 255, 255, 0.32)");
  bloom.addColorStop(1, "rgba(255, 255, 255, 0)");
  context.fillStyle = bloom;
  context.fillRect(0, 0, width, height);

  const windowWidth = Math.min(width * 0.78, 400);
  const windowHeight = Math.min(116, Math.max(76, height * 0.68));
  const windowX = (width - windowWidth) / 2;
  const windowY = 12;
  const radius = 11;
  const windowFill = dark ? "rgba(24, 27, 30, 0.69)" : "rgba(255, 255, 255, 0.68)";
  const windowStroke = dark ? "rgba(242, 244, 247, 0.2)" : "rgba(44, 61, 77, 0.12)";
  context.save();
  context.shadowColor = dark ? "rgba(0, 0, 0, 0.24)" : "rgba(36, 54, 72, 0.13)";
  context.shadowBlur = 18;
  context.shadowOffsetY = 8;
  context.beginPath();
  context.roundRect(windowX, windowY, windowWidth, windowHeight, radius);
  context.fillStyle = windowFill;
  context.fill();
  context.restore();

  context.save();
  context.beginPath();
  context.roundRect(windowX, windowY, windowWidth, windowHeight, radius);
  context.clip();
  context.fillStyle = windowFill;
  context.fillRect(windowX, windowY, windowWidth, windowHeight);
  context.strokeStyle = windowStroke;
  context.lineWidth = 1;
  context.strokeRect(windowX + 0.5, windowY + 0.5, windowWidth - 1, windowHeight - 1);
  context.fillStyle = dark ? "rgba(245, 246, 248, 0.045)" : "rgba(40, 55, 70, 0.025)";
  context.fillRect(windowX, windowY, windowWidth, 23);
  context.strokeStyle = dark ? "rgba(238, 240, 243, 0.1)" : "rgba(36, 53, 69, 0.085)";
  context.beginPath();
  context.moveTo(windowX, windowY + 23.5);
  context.lineTo(windowX + windowWidth, windowY + 23.5);
  context.stroke();

  const dotColors = dark
    ? ["rgba(255, 255, 255, 0.32)", "rgba(255, 255, 255, 0.23)", "rgba(255, 255, 255, 0.16)"]
    : ["rgba(82, 96, 110, 0.38)", "rgba(82, 96, 110, 0.26)", "rgba(82, 96, 110, 0.18)"];
  dotColors.forEach((color, index) => {
    context.beginPath();
    context.arc(windowX + 13 + index * 9, windowY + 12, 2.5, 0, Math.PI * 2);
    context.fillStyle = color;
    context.fill();
  });

  const lineColor = dark ? "rgba(232, 234, 237, 0.15)" : "rgba(47, 64, 80, 0.12)";
  const titleColor = dark ? "rgba(241, 243, 245, 0.24)" : "rgba(51, 68, 84, 0.2)";
  context.fillStyle = titleColor;
  context.fillRect(windowX + 15, windowY + 39, Math.min(104, windowWidth * 0.32), 5);
  context.fillStyle = lineColor;
  context.fillRect(windowX + 15, windowY + 55, windowWidth * 0.73, 3);
  context.fillRect(windowX + 15, windowY + 65, windowWidth * 0.61, 3);
  context.fillRect(windowX + 15, windowY + 75, windowWidth * 0.68, 3);
  context.fillRect(windowX + 15, windowY + 85, windowWidth * 0.42, 3);

  const chartX = windowX + windowWidth * 0.75;
  const chartY = windowY + 44;
  const chartWidth = Math.max(24, windowWidth * 0.19);
  const chartHeight = Math.max(20, windowHeight - 58);
  context.strokeStyle = dark ? "rgba(176, 186, 198, 0.3)" : "rgba(92, 144, 181, 0.24)";
  context.lineWidth = 1.5;
  context.beginPath();
  context.moveTo(chartX, chartY + chartHeight * 0.78);
  context.lineTo(chartX + chartWidth * 0.24, chartY + chartHeight * 0.58);
  context.lineTo(chartX + chartWidth * 0.46, chartY + chartHeight * 0.68);
  context.lineTo(chartX + chartWidth * 0.7, chartY + chartHeight * 0.27);
  context.lineTo(chartX + chartWidth, chartY + chartHeight * 0.18);
  context.stroke();
  context.restore();

  return true;
}
