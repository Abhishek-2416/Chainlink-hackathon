export const WALLET_ICONS: Record<string, string> = {
  metamask:
    "https://www.pngall.com/wp-content/uploads/17/Metamask-Financial-Services-Logo-PNG-thumb.png",
  "meta mask":
    "https://www.pngall.com/wp-content/uploads/17/Metamask-Financial-Services-Logo-PNG-thumb.png",
  coinbase:
    "https://raw.githubusercontent.com/gist/taycaldwell/2291907115c0bb5589bc346661435007/raw/280eafdc84cb80ed0c60e36b4d0c563f6dca6b3e/cbw.svg",
  "coinbase wallet":
    "https://raw.githubusercontent.com/gist/taycaldwell/2291907115c0bb5589bc346661435007/raw/280eafdc84cb80ed0c60e36b4d0c563f6dca6b3e/cbw.svg",
  walletconnect: "https://avatars.githubusercontent.com/u/37784886?s=200&v=4",
  injected:
    "https://upload.wikimedia.org/wikipedia/commons/7/70/Ethereum_logo.svg",
  "browser wallet":
    "https://upload.wikimedia.org/wikipedia/commons/7/70/Ethereum_logo.svg",
  rabby:
    "https://play-lh.googleusercontent.com/voFLXuFxLsIFBHQKmFxUhgAo23RXmO6_esdEb6ebfHQewdMlAfNKq3vAaDh6clJ7Pw",
  "rabby wallet":
    "https://play-lh.googleusercontent.com/voFLXuFxLsIFBHQKmFxUhgAo23RXmO6_esdEb6ebfHQewdMlAfNKq3vAaDh6clJ7Pw",
  core: "https://build.avax.network/images/core.svg",
  phantom:
    "https://cdn.prod.website-files.com/6410de4b1ee56e7333393b23/66d87fb4733b331acc81216e_Phantom-Icon_Transparent_Purple.png",
  keplr:
    "https://play-lh.googleusercontent.com/q3IAZGlrfKwt-IxX3WWcWJzah56y2RqhESi3Xk8hFarVNnbPtzLSgRDI2JV1681pf2sq5e2lr17ZVD-wzV77IGk",
  leap:
    "https://play-lh.googleusercontent.com/0BY-XzNk_6R3DS_oNZfRI-x5L2PDgX8BDo7OL8kPDCKaQi0YzXGrYKWaT2lbOkqqGrs=w240-h480-rw",
  okx: "https://play-lh.googleusercontent.com/N00SbjLJJrhg4hbdnkk3Llk2oedNNgCU29DvR9cpep7Lr0VkzvBkmLqajWNgFb0d7IOO=w240-h480-rw",
  "okx wallet":
    "https://play-lh.googleusercontent.com/N00SbjLJJrhg4hbdnkk3Llk2oedNNgCU29DvR9cpep7Lr0VkzvBkmLqajWNgFb0d7IOO=w240-h480-rw",
  unisat: "https://static.images.dropstab.com/images/unisat.png",
  "unisat wallet": "https://static.images.dropstab.com/images/unisat.png",
  xdefi:
    "https://moralis.com/wp-content/uploads/web3wiki/1276-xdefi-wallet/63a46c480b012fc7f5436808_Mb-VXGh_QAeZeuXsT43JUNAYIyh3tn1YeRCfQmVdc08.png",
};

export function getWalletIcon(connectorName: string): string | undefined {
  if (!connectorName) return undefined;
  const name = connectorName.toLowerCase().trim();
  if (WALLET_ICONS[name]) return WALLET_ICONS[name];
  const matchingKey = Object.keys(WALLET_ICONS).find(
    (key) => name.includes(key) || key.includes(name)
  );
  return matchingKey ? WALLET_ICONS[matchingKey] : undefined;
}
