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
  float amount = uShape.z * rim * pow(1.0 - depth, uShape.w);
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
    kind?: "light" | "dark";
    keep?: number;
    rim?: number;
    edge?: number;
    power?: number;
    dispersion?: number;
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
      render({
        source,
        scale,
        offsetX,
        offsetY,
        sceneWidth,
        sceneHeight,
        width,
        height,
        kind = "dark",
        keep = 1,
        rim = 40,
        edge = 1.1,
        power = 2.6,
        dispersion = 0.15,
      }) {
        if (disposed || gl.isContextLost() || !source.width || !source.height || !width || !height) return false;
        try {
          const radius = Math.min(24, width / 2, height / 2);
          const bezel = Math.max(4, Math.min(rim, Math.min(width, height) / 3.5));
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
          gl.uniform4f(uniforms.shape, radius, bezel, edge, power);
          if (kind === "dark") {
            gl.uniform4f(uniforms.tone, 2.0, 1.6, 1.0, 0.8);
          } else {
            gl.uniform4f(uniforms.tone, 2.0, 1.8, 1.0, 1.06);
          }
          gl.uniform2f(uniforms.more, keep, dispersion);
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

function mixChannel(light: number, dark: number, progress: number) {
  return Math.round(light + (dark - light) * Math.min(1, Math.max(0, progress)));
}

export function drawGlassworkScene(
  canvas: HTMLCanvasElement,
  width: number,
  height: number,
  scale: number,
  backdrop: "light" | "dark",
  darkProgress?: number,
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

  const progress = darkProgress ?? (backdrop === "dark" ? 1 : 0);
  if (progress <= 0) {
    context.fillStyle = "#f2f3f5";
  } else if (progress >= 1) {
    context.fillStyle = "#121212";
  } else {
    const r = mixChannel(242, 18, progress);
    const g = mixChannel(243, 18, progress);
    const b = mixChannel(245, 18, progress);
    context.fillStyle = `rgb(${r} ${g} ${b})`;
  }
  context.fillRect(0, 0, width, height);

  return true;
}
