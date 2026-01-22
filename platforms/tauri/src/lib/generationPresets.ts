import type { LucideIcon } from 'lucide-react';
import {
  Image,
  Video,
  Music,
  Sparkles,
  Zap,
  Palette,
  Wand2,
  Film,
  Mic,
  Radio,
  Clapperboard,
  Camera,
  SpeakerIcon as Speaker,
  Layers,
  Bot,
} from 'lucide-react';

export type GenerationCategory = 'image' | 'video' | 'audio';

export interface GenerationPresetProvider {
  id: string;
  name: string;
  icon: LucideIcon;
  color: string;
  type: string;
  defaultModel: string;
  description: string;
  baseUrl?: string;
  category: GenerationCategory;
  isUnsupported?: boolean;
}

// Image Generation Providers
export const imageProviders: GenerationPresetProvider[] = [
  {
    id: 'openai-dalle',
    name: 'DALL-E',
    icon: Sparkles,
    color: '#10a37f',
    type: 'openai',
    defaultModel: 'dall-e-3',
    description: 'OpenAI image generation',
    category: 'image',
  },
  {
    id: 'stability-ai',
    name: 'Stability AI',
    icon: Palette,
    color: '#7c3aed',
    type: 'stability',
    defaultModel: 'stable-diffusion-xl-1024-v1-0',
    description: 'Stable Diffusion models',
    baseUrl: 'https://api.stability.ai',
    category: 'image',
  },
  {
    id: 'midjourney',
    name: 'Midjourney',
    icon: Wand2,
    color: '#1a1a2e',
    type: 'midjourney',
    defaultModel: 'midjourney-v6',
    description: 'Midjourney image generation',
    category: 'image',
    isUnsupported: true,
  },
  {
    id: 'flux',
    name: 'Flux',
    icon: Layers,
    color: '#ff6b6b',
    type: 'replicate',
    defaultModel: 'flux-1.1-pro',
    description: 'Black Forest Labs Flux models',
    baseUrl: 'https://api.replicate.com/v1',
    category: 'image',
  },
  {
    id: 'ideogram',
    name: 'Ideogram',
    icon: Image,
    color: '#0ea5e9',
    type: 'ideogram',
    defaultModel: 'ideogram-v2',
    description: 'Text-to-image with typography',
    baseUrl: 'https://api.ideogram.ai',
    category: 'image',
  },
  {
    id: 'leonardo',
    name: 'Leonardo.AI',
    icon: Camera,
    color: '#f97316',
    type: 'leonardo',
    defaultModel: 'leonardo-diffusion-xl',
    description: 'Creative AI image generation',
    baseUrl: 'https://cloud.leonardo.ai/api',
    category: 'image',
  },
];

// Video Generation Providers
export const videoProviders: GenerationPresetProvider[] = [
  {
    id: 'runway',
    name: 'Runway',
    icon: Film,
    color: '#6366f1',
    type: 'runway',
    defaultModel: 'gen-3-alpha',
    description: 'AI video generation',
    baseUrl: 'https://api.runwayml.com',
    category: 'video',
  },
  {
    id: 'pika',
    name: 'Pika',
    icon: Video,
    color: '#22c55e',
    type: 'pika',
    defaultModel: 'pika-1.0',
    description: 'Text-to-video generation',
    category: 'video',
    isUnsupported: true,
  },
  {
    id: 'kling',
    name: 'Kling',
    icon: Clapperboard,
    color: '#3b82f6',
    type: 'kling',
    defaultModel: 'kling-v1',
    description: 'Kuaishou video generation',
    category: 'video',
  },
  {
    id: 'luma',
    name: 'Luma Dream Machine',
    icon: Zap,
    color: '#8b5cf6',
    type: 'luma',
    defaultModel: 'dream-machine',
    description: 'Realistic video generation',
    baseUrl: 'https://api.lumalabs.ai',
    category: 'video',
  },
  {
    id: 'minimax',
    name: 'MiniMax',
    icon: Bot,
    color: '#ec4899',
    type: 'minimax',
    defaultModel: 'video-01',
    description: 'MiniMax video generation',
    baseUrl: 'https://api.minimax.chat',
    category: 'video',
  },
];

// Audio Generation Providers
export const audioProviders: GenerationPresetProvider[] = [
  {
    id: 'elevenlabs',
    name: 'ElevenLabs',
    icon: Mic,
    color: '#f59e0b',
    type: 'elevenlabs',
    defaultModel: 'eleven_multilingual_v2',
    description: 'AI voice synthesis',
    baseUrl: 'https://api.elevenlabs.io',
    category: 'audio',
  },
  {
    id: 'openai-tts',
    name: 'OpenAI TTS',
    icon: Speaker,
    color: '#10a37f',
    type: 'openai',
    defaultModel: 'tts-1-hd',
    description: 'OpenAI text-to-speech',
    category: 'audio',
  },
  {
    id: 'suno',
    name: 'Suno',
    icon: Music,
    color: '#ef4444',
    type: 'suno',
    defaultModel: 'suno-v3.5',
    description: 'AI music generation',
    category: 'audio',
    isUnsupported: true,
  },
  {
    id: 'udio',
    name: 'Udio',
    icon: Radio,
    color: '#14b8a6',
    type: 'udio',
    defaultModel: 'udio-v1',
    description: 'AI music composition',
    category: 'audio',
    isUnsupported: true,
  },
];

// All generation providers grouped
export const generationProviders = {
  image: imageProviders,
  video: videoProviders,
  audio: audioProviders,
};

// Get all providers as a flat array
export const allGenerationProviders: GenerationPresetProvider[] = [
  ...imageProviders,
  ...videoProviders,
  ...audioProviders,
];

// Category metadata
export const categoryMeta: Record<
  GenerationCategory,
  { icon: LucideIcon; labelKey: string; color: string }
> = {
  image: { icon: Image, labelKey: 'settings.generationProviders.categories.image', color: '#8b5cf6' },
  video: { icon: Video, labelKey: 'settings.generationProviders.categories.video', color: '#3b82f6' },
  audio: { icon: Music, labelKey: 'settings.generationProviders.categories.audio', color: '#f59e0b' },
};
