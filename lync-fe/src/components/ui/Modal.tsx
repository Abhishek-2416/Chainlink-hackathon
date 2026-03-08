import { type ReactNode, useState, useEffect } from "react";
import { createPortal } from "react-dom";
import { motion, AnimatePresence, useDragControls } from "framer-motion";
import { clsx } from "clsx";

const DRAG_DISMISS_THRESHOLD = 80;

function useIsMobile() {
  const [isMobile, setIsMobile] = useState(false);
  useEffect(() => {
    const m = window.matchMedia("(max-width: 767px)");
    setIsMobile(m.matches);
    const fn = () => setIsMobile(m.matches);
    m.addEventListener("change", fn);
    return () => m.removeEventListener("change", fn);
  }, []);
  return isMobile;
}

interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  children: ReactNode;
  size?: "sm" | "md" | "lg" | "full";
  height?: string;
  className?: string;
}

const sizeClasses = {
  sm: "max-w-sm",
  md: "max-w-md",
  lg: "max-w-lg",
  full: "max-w-4xl",
};

export function Modal({
  isOpen,
  onClose,
  title,
  children,
  size = "md",
  height,
  className,
}: ModalProps) {
  const isMobile = useIsMobile();
  const dragControls = useDragControls();
  if (typeof document === "undefined") return null;

  const handleDragEnd = (_: MouseEvent | TouchEvent | PointerEvent, info: { offset: { y: number }; velocity: { y: number } }) => {
    if (info.offset.y > DRAG_DISMISS_THRESHOLD || info.velocity.y > 300) {
      onClose();
    }
  };

  const mobileVariants = {
    initial: { opacity: 0, y: "100%" },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, y: "100%" },
  };
  const desktopVariants = {
    initial: { opacity: 0, scale: 0.95 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, scale: 0.95 },
  };
  const variants = isMobile ? mobileVariants : desktopVariants;

  return createPortal(
    <AnimatePresence>
      {isOpen && (
        <>
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="fixed inset-0 z-40 bg-black/60 backdrop-blur-sm"
            onClick={onClose}
          />
          <div className="fixed inset-0 z-50 flex items-end justify-center p-0 md:items-center md:p-4">
            <motion.div
              initial={variants.initial}
              animate={variants.animate}
              exit={variants.exit}
              transition={{ type: "spring", damping: 25, stiffness: 300 }}
              drag={isMobile ? "y" : false}
              dragControls={isMobile ? dragControls : undefined}
              dragConstraints={isMobile ? { top: 0, bottom: 400 } : undefined}
              dragElastic={isMobile ? { top: 0, bottom: 0.5 } : undefined}
              onDragEnd={isMobile ? handleDragEnd : undefined}
              className={clsx(
                "relative w-full bg-card border border-border shadow-xl flex flex-col",
                "max-h-[90vh] rounded-t-2xl md:max-h-[85vh] md:rounded-xl",
                sizeClasses[size],
                className
              )}
              style={height ? { maxHeight: height } : undefined}
              onClick={(e) => e.stopPropagation()}
            >
              {title && (
                <div
                  className={clsx(
                    "flex shrink-0 items-center border-b border-border px-4 py-3",
                    isMobile ? "justify-center pt-6" : "justify-between"
                  )}
                >
                  {isMobile && (
                    <div
                      className="absolute left-1/2 top-2 -translate-x-1/2 w-12 h-1 rounded-full bg-white/30 cursor-grab active:cursor-grabbing touch-none"
                      onPointerDown={(e) => dragControls.start(e)}
                      aria-hidden
                    />
                  )}
                  <h2 className="text-lg font-semibold text-white">{title}</h2>
                  <button
                    type="button"
                    onClick={onClose}
                    className={clsx(
                      "rounded p-1 text-muted-foreground hover:bg-white/10 hover:text-white",
                      isMobile && "hidden"
                    )}
                    aria-label="Close"
                  >
                    <span className="text-xl leading-none">×</span>
                  </button>
                </div>
              )}
              <div className="flex-1 min-h-0 overflow-y-auto overscroll-contain p-4">
                {children}
              </div>
            </motion.div>
          </div>
        </>
      )}
    </AnimatePresence>,
    document.body
  );
}
