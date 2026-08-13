import { useRef, useState, useEffect } from "react";

interface SkinHeadProps {
  skinUrl: string;
  size?: number;
  className?: string;
}

/**
 * Renders the head portion of a Minecraft skin texture as a thumbnail.
 * Extracts the 8x8 head pixels from the skin texture and renders them
 * with the overlay layer on top.
 */
export function SkinHead({ skinUrl, size = 44, className }: SkinHeadProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!canvasRef.current || !skinUrl) {
      return;
    }

    const canvas = canvasRef.current;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    setLoaded(false);

    const img = new Image();
    // Only set crossOrigin for http(s) URLs, not for asset:// protocol
    if (skinUrl.startsWith("http")) {
      img.crossOrigin = "anonymous";
    }
    img.onload = () => {
      // Clear canvas
      ctx.clearRect(0, 0, size, size);

      // Minecraft skin head is at (8, 8) with size 8x8 pixels
      // Draw the base head layer
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(img, 8, 8, 8, 8, 0, 0, size, size);

      // Draw the overlay layer (at 40, 8)
      ctx.drawImage(img, 40, 8, 8, 8, 0, 0, size, size);
      setLoaded(true);
    };
    img.onerror = () => {
      // On error, show a placeholder
      ctx.fillStyle = "#333";
      ctx.fillRect(0, 0, size, size);
    };
    img.src = skinUrl;
  }, [skinUrl, size]);

  return (
    <canvas
      ref={canvasRef}
      width={size}
      height={size}
      className={className}
      style={{ imageRendering: "pixelated", borderRadius: size > 40 ? 10 : 6, opacity: loaded ? 1 : 0.5 }}
    />
  );
}

interface SkinPreviewProps {
  skinUrl: string;
  width?: number;
  height?: number;
}

/**
 * Renders a simple static front-view preview of a Minecraft skin.
 * More performant than a full 3D viewer, suitable for library cards.
 */
export function SkinPreview({ skinUrl, width = 60, height = 90 }: SkinPreviewProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState(false);

  useEffect(() => {
    if (!canvasRef.current || !skinUrl) {
      setError(true);
      return;
    }

    const canvas = canvasRef.current;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    setLoaded(false);
    setError(false);

    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => {
      ctx.clearRect(0, 0, width, height);
      ctx.imageSmoothingEnabled = false;

      // Scale to fit - skin preview showing front view
      const scale = Math.min(width / 16, height / 32) * 0.85;
      const offsetX = (width - 16 * scale) / 2;
      const offsetY = (height - 32 * scale) / 2;

      // Draw head (8x8 at position 8,8)
      ctx.drawImage(img, 8, 8, 8, 8, offsetX + 4 * scale, offsetY, 8 * scale, 8 * scale);
      // Draw head overlay
      ctx.drawImage(img, 40, 8, 8, 8, offsetX + 4 * scale, offsetY, 8 * scale, 8 * scale);

      // Draw body (8x12 at position 20,20)
      ctx.drawImage(img, 20, 20, 8, 12, offsetX + 4 * scale, offsetY + 8 * scale, 8 * scale, 12 * scale);

      // Draw right arm (4x12 at position 44,20)
      ctx.drawImage(img, 44, 20, 4, 12, offsetX, offsetY + 8 * scale, 4 * scale, 12 * scale);

      // Draw left arm (4x12 at position 36,52)
      ctx.drawImage(img, 36, 52, 4, 12, offsetX + 12 * scale, offsetY + 8 * scale, 4 * scale, 12 * scale);

      // Draw right leg (4x12 at position 4,20)
      ctx.drawImage(img, 4, 20, 4, 12, offsetX + 4 * scale, offsetY + 20 * scale, 4 * scale, 12 * scale);

      // Draw left leg (4x12 at position 20,52)
      ctx.drawImage(img, 20, 52, 4, 12, offsetX + 8 * scale, offsetY + 20 * scale, 4 * scale, 12 * scale);

      setLoaded(true);
    };
    img.onerror = () => {
      setError(true);
    };
    img.src = skinUrl;
  }, [skinUrl, width, height]);

  if (error || !skinUrl) {
    return (
      <div className="skin-preview-error" style={{ width, height, display: "flex", alignItems: "center", justifyContent: "center" }}>
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" opacity="0.3">
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <path d="M9 9h.01M15 9h.01M9 15h6" />
        </svg>
      </div>
    );
  }

  return (
    <canvas
      ref={canvasRef}
      width={width}
      height={height}
      style={{
        imageRendering: "pixelated",
        opacity: loaded ? 1 : 0.5,
        transition: "opacity 0.2s ease"
      }}
    />
  );
}
