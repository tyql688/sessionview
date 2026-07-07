import { useState, useEffect } from "react";
import { XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { readImageBase64 } from "@/lib/tauri";
import { cachedLoad } from "@/lib/image-cache";
import { shortenHomePath } from "@/lib/formatters";
import { useI18n } from "@/i18n/index";

export function isLocalPath(source: string): boolean {
  return (
    !source.startsWith("data:") &&
    !source.startsWith("http://") &&
    !source.startsWith("https://") &&
    !source.startsWith("asset:")
  );
}

export function LocalImage(props: {
  path: string;
  onPreview: (src: string, source: string) => void;
}) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    setSrc(null);
    setFailed(false);

    cachedLoad(props.path, () => readImageBase64(props.path))
      .then((loaded) => {
        if (!active) return;
        setSrc(loaded);
      })
      .catch((e) => {
        if (!active) return;
        console.warn("failed to load image:", props.path, e);
        setFailed(true);
      });

    return () => {
      active = false;
    };
  }, [props.path]);

  if (src) {
    return (
      <InlineImage src={src} source={props.path} onPreview={props.onPreview} />
    );
  }

  return failed ? (
    <div className="msg-image-wrap">
      <ImageFallback source={props.path} />
    </div>
  ) : (
    <div className="msg-image-wrap">
      <ImageLoading source={props.path} />
    </div>
  );
}

export function RemoteImage(props: {
  src: string;
  onPreview: (src: string, source: string) => void;
}) {
  const [loadedSrc, setLoadedSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    setLoadedSrc(null);
    setFailed(false);

    cachedLoad(props.src, () => {
      return new Promise<string>((resolve, reject) => {
        const image = new Image();
        image.onload = () => resolve(props.src);
        image.onerror = () => reject(new Error("remote image load failed"));
        image.src = props.src;
      });
    })
      .then((src) => {
        if (!active) return;
        setLoadedSrc(src);
      })
      .catch(() => {
        if (!active) return;
        setFailed(true);
      });

    return () => {
      active = false;
    };
  }, [props.src]);

  if (loadedSrc) {
    return (
      <InlineImage
        src={loadedSrc}
        source={props.src}
        onPreview={props.onPreview}
      />
    );
  }

  return failed ? (
    <div className="msg-image-wrap">
      <ImageFallback source={props.src} />
    </div>
  ) : (
    <div className="msg-image-wrap">
      <ImageLoading source={props.src} />
    </div>
  );
}

function InlineImage(props: {
  src: string;
  source: string;
  onPreview: (src: string, source: string) => void;
}) {
  return (
    <div className="msg-image-wrap">
      <Button
        variant="ghost"
        type="button"
        className="msg-image-button h-auto p-0 active:translate-y-0"
        onClick={() => props.onPreview(props.src, props.source)}
        title={describeImageSource(props.source)}
      >
        <img
          src={props.src}
          alt={describeImageSource(props.source)}
          className="msg-image is-ready"
          loading="lazy"
          decoding="async"
          draggable={false}
        />
      </Button>
    </div>
  );
}

function ImageLoading(props: { source: string }) {
  const { t } = useI18n();
  return (
    <div
      className="msg-image-state msg-image-loading"
      title={describeImageSource(props.source)}
    >
      <span className="msg-image-state-label">{t("common.loading")}</span>
      <span className="msg-image-state-source">
        {labelImageSource(props.source, t)}
      </span>
    </div>
  );
}

function ImageFallback(props: { source: string }) {
  const { t } = useI18n();
  return (
    <div
      className="msg-image-state msg-image-fallback"
      title={describeImageSource(props.source)}
    >
      <span className="msg-image-state-label">
        {t("common.imageLoadFailed")}
      </span>
      <span className="msg-image-state-source">
        {labelImageSource(props.source, t)}
      </span>
    </div>
  );
}

export function ImagePreview(props: {
  src: string;
  source?: string;
  onClose: () => void;
}) {
  const { t } = useI18n();

  return (
    <Dialog open={true} onOpenChange={(open) => !open && props.onClose()}>
      <DialogContent
        showCloseButton={false}
        className="top-0 left-0 flex h-dvh w-dvw max-w-none translate-x-0 translate-y-0 items-center justify-center gap-0 rounded-none bg-transparent p-0 shadow-none ring-0 sm:max-w-none"
      >
        <DialogTitle className="sr-only">{t("common.image")}</DialogTitle>
        <div className="image-preview-stage" onClick={props.onClose}>
          <img
            src={props.src}
            alt={t("common.image")}
            className="image-preview-img"
            onClick={(e) => e.stopPropagation()}
          />
          {props.source && (
            <div
              className="image-preview-meta"
              title={describeImageSource(props.source)}
              onClick={(e) => e.stopPropagation()}
            >
              {labelImageSource(props.source, t)}
            </div>
          )}
        </div>
        <Button
          variant="ghost"
          size="icon-lg"
          type="button"
          className="image-preview-close active:translate-y-0"
          aria-label={t("common.closePreview")}
          onClick={props.onClose}
        >
          <XIcon className="size-5" aria-hidden="true" />
        </Button>
      </DialogContent>
    </Dialog>
  );
}

function labelImageSource(source: string, t: (key: string) => string): string {
  if (source.startsWith("data:")) {
    return t("common.embeddedImage");
  }

  if (source.startsWith("http://") || source.startsWith("https://")) {
    try {
      const url = new URL(source);
      return url.toString();
    } catch (error) {
      console.warn("Failed to parse image source URL:", error);
      return source;
    }
  }

  const normalized = shortenHomePath(source).replace(/\\/g, "/");
  return normalized || source;
}

function describeImageSource(source: string): string {
  if (source.startsWith("data:")) {
    return "embedded image";
  }
  return shortenHomePath(source).replaceAll("\\", "/");
}
