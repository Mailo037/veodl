// SHA-256 and byte counts pinned at development time from upstream GitHub assets.
// https://github.com/eugeneware/ffmpeg-static/releases/tag/b6.1.1
// Windows ia32 uses b6.0 with measured download hashes and its Windows license.
// Never fetch replacement checksum metadata at runtime.
export const MEDIA_ASSET_BASE = 'https://github.com/eugeneware/ffmpeg-static/releases/download';
const freeze = value => { for (const child of Object.values(value)) if (child && typeof child === "object") freeze(child); return Object.freeze(value); };
export const MEDIA_ASSETS = freeze({
  "win32-x64": {
    "release": "b6.1.1",
    "ffmpeg": {
      "name": "ffmpeg-win32-x64",
      "sha256": "04e1307997530f9cf2fe35cba2ca7e8875ca91da02f89d6c7243df819c94ad00",
      "size": 82797568,
      "gzip": {
        "name": "ffmpeg-win32-x64.gz",
        "sha256": "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77",
        "size": 29581307
      }
    },
    "ffprobe": {
      "name": "ffprobe-win32-x64",
      "sha256": "3a7e2dc003dc2cd1472827e4c7c4f056ae1ae0ae7c5bbc580c99b49827351ba4",
      "size": 82668032,
      "gzip": {
        "name": "ffprobe-win32-x64.gz",
        "sha256": "f309e6223ad89d2fe54bccd420a7709b66fd27540674e92309578ed491a43c8d",
        "size": 29521644
      }
    },
    "license": {
      "name": "win32-x64.LICENSE",
      "sha256": "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
      "size": 35147
    },
    "readme": {
      "name": "win32-x64.README",
      "sha256": "a636a7183c58006351acbaf35303c0ed85c6e1320fd4e80de453ba6157de6311",
      "size": 39494
    }
  },
  "darwin-x64": {
    "release": "b6.1.1",
    "ffmpeg": {
      "name": "ffmpeg-darwin-x64",
      "sha256": "ebdddc936f61e14049a2d4b549a412b8a40deeff6540e58a9f2a2da9e6b18894",
      "size": 78862176,
      "gzip": {
        "name": "ffmpeg-darwin-x64.gz",
        "sha256": "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106",
        "size": 25296431
      }
    },
    "ffprobe": {
      "name": "ffprobe-darwin-x64",
      "sha256": "fa3add0ce901f7241abe0dfc0155d958fc834aca3f8ce61f87cc712ae669c1e0",
      "size": 78780408,
      "gzip": {
        "name": "ffprobe-darwin-x64.gz",
        "sha256": "d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186",
        "size": 25239438
      }
    },
    "license": {
      "name": "darwin-x64.LICENSE",
      "sha256": "2e1d16c72fd74e12063776371da757322f8b77589386532f4fd8634bde7de1af",
      "size": 4346
    },
    "readme": {
      "name": "darwin-x64.README",
      "sha256": "e88a0325f8e5b75210355e37341824f074d3cd82def2125be54c914b62848a36",
      "size": 6227
    }
  },
  "darwin-arm64": {
    "release": "b6.1.1",
    "ffmpeg": {
      "name": "ffmpeg-darwin-arm64",
      "sha256": "a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584",
      "size": 45568216,
      "gzip": {
        "name": "ffmpeg-darwin-arm64.gz",
        "sha256": "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa",
        "size": 19246198
      }
    },
    "ffprobe": {
      "name": "ffprobe-darwin-arm64",
      "sha256": "bb2db6f5d8cef919da12fbf592119a987202a8c060a886f3cab091f9cab90b64",
      "size": 45528808,
      "gzip": {
        "name": "ffprobe-darwin-arm64.gz",
        "sha256": "d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be",
        "size": 19207077
      }
    },
    "license": {
      "name": "darwin-arm64.LICENSE",
      "sha256": "cb48bf09a11f5fb576cddb0431c8f5ed0a60157a9ec942adffc13907cbe083f2",
      "size": 4376
    },
    "readme": {
      "name": "darwin-arm64.README",
      "sha256": "05ba4b92c96605434b1aaae3eedf5a2c280c9607bf78ffca9a5b536d9af2dc6a",
      "size": 1810
    }
  },
  "linux-x64": {
    "release": "b6.1.1",
    "ffmpeg": {
      "name": "ffmpeg-linux-x64",
      "sha256": "e7e7fb30477f717e6f55f9180a70386c62677ef8a4d4d1a5d948f4098aa3eb99",
      "size": 79826272,
      "gzip": {
        "name": "ffmpeg-linux-x64.gz",
        "sha256": "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa",
        "size": 29354986
      }
    },
    "ffprobe": {
      "name": "ffprobe-linux-x64",
      "sha256": "4f231a1960d83e403d08f7971e271707bec278a9ae18e21b8b5b03186668450d",
      "size": 79665792,
      "gzip": {
        "name": "ffprobe-linux-x64.gz",
        "sha256": "25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067",
        "size": 29276839
      }
    },
    "license": {
      "name": "linux-x64.LICENSE",
      "sha256": "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
      "size": 35147
    },
    "readme": {
      "name": "linux-x64.README",
      "sha256": "72f4b1b06d419d22ace6e7cc75f06826f90737345aa0b1736158929f4aacc537",
      "size": 2235
    }
  },
  "linux-ia32": {
    "release": "b6.1.1",
    "ffmpeg": {
      "name": "ffmpeg-linux-ia32",
      "sha256": "c6472eb993612db72ca50893a34137ba11173e60a1a4c028d4660a3f755d2490",
      "size": 52155436,
      "gzip": {
        "name": "ffmpeg-linux-ia32.gz",
        "sha256": "169b27c078a8ecedb814cac67afccf15a9868d63e9d74ef86088adefaa500d00",
        "size": 22184031
      }
    },
    "ffprobe": {
      "name": "ffprobe-linux-ia32",
      "sha256": "a13b4cce0a4b9bd6714672e4df81076ec06f4c295d798162d48e3de68c781d6d",
      "size": 51955948,
      "gzip": {
        "name": "ffprobe-linux-ia32.gz",
        "sha256": "a75c55bcaad1b0e79f2201d82b9cc43903d950857615f91baaea5ce92d756e63",
        "size": 22093495
      }
    },
    "license": {
      "name": "linux-ia32.LICENSE",
      "sha256": "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
      "size": 35147
    },
    "readme": {
      "name": "linux-ia32.README",
      "sha256": "3bbce64eba997007b3fd5324bb105f10ab6b0a499796c7d34b844b51fda0528b",
      "size": 2209
    }
  },
  "linux-arm64": {
    "release": "b6.1.1",
    "ffmpeg": {
      "name": "ffmpeg-linux-arm64",
      "sha256": "6bb182d0d75d23028db82e9e4f723ca69b853d055698486e6984ddb2c06fb8ce",
      "size": 51134160,
      "gzip": {
        "name": "ffmpeg-linux-arm64.gz",
        "sha256": "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0",
        "size": 25568691
      }
    },
    "ffprobe": {
      "name": "ffprobe-linux-arm64",
      "sha256": "d17ae9b4c297d48e2521ba14e417bb0537c6ff77c584cdbcd6bb0d8d0307a2e8",
      "size": 50994160,
      "gzip": {
        "name": "ffprobe-linux-arm64.gz",
        "sha256": "2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8",
        "size": 25493573
      }
    },
    "license": {
      "name": "linux-arm64.LICENSE",
      "sha256": "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
      "size": 35147
    },
    "readme": {
      "name": "linux-arm64.README",
      "sha256": "d6777d2fd276b23f0ac6666fa619e88ffe4826521881c7ff83836e30cb4acec2",
      "size": 2217
    }
  },
  "linux-arm": {
    "release": "b6.1.1",
    "ffmpeg": {
      "name": "ffmpeg-linux-arm",
      "sha256": "0afba4a11110e6e402053e0fc14c33a7eb207d7a588688ae87dba471a0f06c71",
      "size": 31793580,
      "gzip": {
        "name": "ffmpeg-linux-arm.gz",
        "sha256": "64b115a12f0ab77c277e3c418aae8b40ef881e75e746a0e2d066a206b9bc5172",
        "size": 19167563
      }
    },
    "ffprobe": {
      "name": "ffprobe-linux-arm",
      "sha256": "e61ef24870ddc3b80aa8b435512a545bd889e4ed4c2fc7c885f7d3dabad2a53f",
      "size": 31671692,
      "gzip": {
        "name": "ffprobe-linux-arm.gz",
        "sha256": "2471169c19fea00018413eebf188703c19ae5ab614477465146d4cdc7458b55d",
        "size": 19084901
      }
    },
    "license": {
      "name": "linux-arm.LICENSE",
      "sha256": "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
      "size": 35147
    },
    "readme": {
      "name": "linux-arm.README",
      "sha256": "faaea83a6d92419cb112d5de5ac673f51b72401830e068c4d003b3229ee1c89a",
      "size": 2178
    }
  },
  "win32-ia32": {
    "release": "b6.0",
    "ffprobe": {
      "name": "ffprobe-win32-ia32",
      "sha256": "c1ca7f2c0f93285629360d5afe622080ec4ce53407c427f7b8a48d7e8fcab02d",
      "size": 74125312,
      "gzip": {
        "name": "ffprobe-win32-ia32.gz",
        "sha256": "9ccc2eb5c37f8ba8c4cf8f0a73ecb508b740fa64b0a2c3171acc98f89f409e87",
        "size": 31332907
      }
    },
    "ffmpeg": {
      "name": "ffmpeg-win32-ia32",
      "sha256": "fb3766af5cc193ca863e15cd4554a33732973209dad5e3c1433b5e291bceb16c",
      "size": 74250240,
      "gzip": {
        "name": "ffmpeg-win32-ia32.gz",
        "sha256": "af47ca5de7b9f859f48e4e96ea831bf551b100144b0b871762ab48fba1152f56",
        "size": 31390495
      }
    },
    "license": {
      "name": "win32-x64.LICENSE",
      "sha256": "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
      "size": 35147
    },
    "readme": {
      "name": "win32-x64.README",
      "sha256": "8cf2907e089d95142c752a69cd934956b552e71d8d4762e65dbd227f5e40237e",
      "size": 38167
    }
  }
});
