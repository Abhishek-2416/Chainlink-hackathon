import { useEffect, useState } from "react";
import { baseSepolia } from "wagmi/chains";
import { useAccount, useConnect, useDisconnect } from "wagmi";
import { Modal } from "../ui/Modal";
import { useUIStore } from "../../stores/uiStore";
import { Button } from "../ui/Button";
import { formatCurrency } from "../../utils/format";
import { marketService } from "../../services/marketService";
import { getWalletIcon } from "../../constants/walletIcons";

export function WalletModal() {
  const { openModal, setOpenModal } = useUIStore();
  const { address, isConnected } = useAccount();
  const { connectors, connectAsync, isPending, error } = useConnect();
  const { disconnect } = useDisconnect();
  const [balance, setBalance] = useState<{ eth: number; usdc: number } | null>(null);
  const [connectingId, setConnectingId] = useState<string | null>(null);

  const isOpen = openModal === "wallet";

  useEffect(() => {
    if (address && isOpen) {
      marketService.getBalance(address).then((b) =>
        setBalance(b ? { eth: b.ethFormatted, usdc: b.usdcFormatted } : null)
      );
    } else {
      setBalance(null);
    }
  }, [address, isOpen]);

  const handleDisconnect = () => {
    disconnect();
    setOpenModal(null);
  };

  const handleConnect = async (connector: (typeof connectors)[number]) => {
    setConnectingId(connector.uid);
    try {
      await connectAsync({ connector, chainId: baseSepolia.id });
      setOpenModal(null);
    } catch {
      // Keep modal open on error so user can retry or see error
    } finally {
      setConnectingId(null);
    }
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={() => setOpenModal(null)}
      title="Connect Wallet"
      size="sm"
      height="60vh"
    >
      <div className="space-y-4">
        {isConnected && address ? (
          <>
            <div className="rounded-lg bg-white/5 p-3">
              <p className="text-xs text-muted-foreground">Connected address</p>
              <p className="mt-1 font-mono text-sm text-white break-all">
                {address}
              </p>
            </div>
            <div className="space-y-2">
              <div className="rounded-lg bg-white/5 p-3">
                <p className="text-xs text-muted-foreground">USDC balance</p>
                <p className="mt-1 text-lg font-semibold text-yes">
                  {balance ? formatCurrency(balance.usdc) : "—"}
                </p>
              </div>
              <div className="rounded-lg bg-white/5 p-3">
                <p className="text-xs text-muted-foreground">ETH (gas)</p>
                <p className="mt-1 text-sm text-white">
                  {balance ? formatCurrency(balance.eth) : "—"}
                </p>
              </div>
            </div>
            <Button variant="no" fullWidth onClick={handleDisconnect}>
              Disconnect
            </Button>
          </>
        ) : (
          <>
            <p className="text-sm text-muted-foreground">
              Login or sign up by connecting your wallet. No email or password required.
            </p>
            <div className="space-y-2">
              {connectors.map((connector) => {
                const iconUrl = getWalletIcon(connector.name);
                const isThisConnectorConnecting = connectingId === connector.uid;
                return (
                  <Button
                    key={connector.uid}
                    variant="secondary"
                    fullWidth
                    onClick={() => handleConnect(connector)}
                    disabled={isPending}
                    className="flex items-center justify-start gap-3"
                  >
                    <div className="w-6 h-6 flex items-center justify-center flex-shrink-0 rounded overflow-hidden bg-white/5">
                      {iconUrl ? (
                        <>
                          <img
                            src={iconUrl}
                            alt=""
                            className="w-5 h-5 object-contain"
                            referrerPolicy="no-referrer"
                            onError={(e) => {
                              e.currentTarget.style.display = "none";
                              const parent = e.currentTarget.parentElement;
                              const fallback = parent?.querySelector(".wallet-icon-fallback");
                              if (fallback) (fallback as HTMLElement).style.display = "flex";
                            }}
                          />
                          <span
                            className="wallet-icon-fallback w-5 h-5 flex items-center justify-center text-xs font-semibold text-muted-foreground"
                            style={{ display: "none" }}
                          >
                            {(connector.name || "?")[0]}
                          </span>
                        </>
                      ) : (
                        <span className="w-5 h-5 flex items-center justify-center text-xs font-semibold text-muted-foreground">
                          {(connector.name || "?")[0]}
                        </span>
                      )}
                    </div>
                    {isThisConnectorConnecting ? "Connecting..." : connector.name}
                  </Button>
                );
              })}
            </div>
            {error && (
              <p className="text-sm text-red-400">{error.message}</p>
            )}
            <p className="text-xs text-muted-foreground">
              If the wallet popup doesn&apos;t appear, check if your browser blocked it and allow popups for this site.
            </p>
          </>
        )}
      </div>
    </Modal>
  );
}
