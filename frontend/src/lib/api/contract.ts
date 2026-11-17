import type { z } from 'zod';
import type { Operations } from './contracts.gen.js';

type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
type WithoutQuery<S extends string> = S extends `${infer Path}?${string}` ? Path : S;
type PathValue<P, K extends string> = K extends keyof P ? P[K] : never;
type Segment<Actual extends string, Pattern extends string, P> = Pattern extends `{${infer K}}`
  ? [PathValue<P, K>] extends [never] ? false
    : [PathValue<P, K>] extends [number] ? Actual extends `${number}` ? true : false
    : [PathValue<P, K>] extends [boolean] ? Actual extends `${boolean}` ? true : false
    : true
  : Actual extends Pattern ? true : false;
type Matches<Url extends string, Pattern extends string, P> =
  Pattern extends `${infer Head}/${infer Tail}`
    ? Url extends `${infer Actual}/${infer Rest}`
      ? Segment<Actual, Head, P> extends true
        ? Matches<Rest, Tail, P> : false
      : false
    : Url extends `${string}/${string}` ? false
      : Segment<Url, Pattern, P>;
type Operation<Url extends string, M extends Method> = Url extends unknown ? {
  [K in keyof Operations]: Operations[K]['method'] extends M
    ? Matches<WithoutQuery<Url>, `/api/v1${Operations[K]['data']['url']}`, NonNullable<Operations[K]['data']['path']>> extends true ? Operations[K] : never
    : never
}[keyof Operations] : never;
type Response<Url extends string, M extends Method> = Operation<Url, M>['response'];
type MissingOperation<Url extends string, M extends Method> = Url extends unknown ? M extends unknown
  ? [Operation<Url, M>] extends [never] ? `${M} ${Url}` : never : never : never;
type RequestBody<Url extends string, M extends Method> = Operation<Url, M>['data'] extends { body?: infer B } ? B : never;
type Data<Url extends string, M extends Method> = Operation<Url, M>['data'];
type BodyValue<B> = JsonBody<B> | (B extends Blob ? B : never);
type BodyInit<Url extends string, M extends Method> = Data<Url, M> extends { body: infer B } ? { body: BodyValue<B> }
  : { body?: BodyValue<RequestBody<Url, M>> };
type QueryInit<Url extends string, M extends Method> = Data<Url, M> extends { query: infer Q } ? { query: ApiQuery<Q> }
  : Data<Url, M> extends { query?: infer Q } ? { query?: ApiQuery<NonNullable<Q>> } : { query?: never };
export type ContractPath<Url extends string, M extends Method> = Url &
  ([Extract<Url, `${string}?${string}`>] extends [never]
    ? [MissingOperation<Url, M>] extends [never] ? unknown : { readonly __unknown_api_operation: never }
    : { readonly __use_typed_query_options: never });

declare const jsonPayload: unique symbol;
declare const jsonKeys: unique symbol;
declare const queryKeys: unique symbol;
export type JsonBody<T> = string & { readonly [jsonPayload]: T; readonly [jsonKeys]: Keys<T> };
export type ApiQuery<T> = T & { readonly [queryKeys]: Exclude<Keys<T>, symbol> };
export type ContractInit<Url extends string, M extends Method> = Omit<RequestInit, 'body' | 'method'> & {
  method?: M;
} & BodyInit<Url, M> & QueryInit<Url, M>;
export type ContractArgs<Url extends string, M extends Method> = {} extends ContractInit<Url, M>
  ? [init?: ContractInit<Url, M>] : [init: ContractInit<Url, M>];
type Keys<T> = T extends unknown ? keyof T : never;
type Field<T, K extends PropertyKey> = T extends unknown ? K extends keyof T ? T[K] : never : never;
type RequiredKeys<T> = T extends unknown ? { [K in keyof T]-?: undefined extends T[K] ? never : K }[keyof T] : never;
type WireShape<A, B, Depth extends number[] = []> = Depth['length'] extends 8 ? true
  : unknown extends A ? true : unknown extends B ? true
  : null extends A ? null extends B ? WireValue<NonNullable<A>, NonNullable<B>, Depth> : false
  : WireValue<NonNullable<A>, NonNullable<B>, Depth>;
type WireValue<A, B, Depth extends number[]> = [A] extends [readonly (infer Item)[]]
  ? [B] extends [readonly (infer Other)[]] ? WireShape<Item, Other, [...Depth, 0]> : false
  : [A] extends [object]
    ? string extends Keys<A> ? true
      : false extends { [K in Keys<A> & Keys<B>]: WireShape<Field<A, K>, Field<B, K>, [...Depth, 0]> }[Keys<A> & Keys<B>] ? false : true
    : [A] extends [string] ? string extends A ? true : [A] extends [B] ? true : false
    : [A] extends [number] ? number extends A ? true : [A] extends [B] ? true : false
    : [A] extends [boolean] ? boolean extends A ? true : [A] extends [B] ? true : false
    : true;
type SameKeys<A, B, Depth extends number[] = []> = Depth['length'] extends 8 ? true
  : unknown extends A ? true
  : NonNullable<A> extends readonly (infer Item)[]
    ? NonNullable<B> extends readonly (infer Other)[] ? SameKeys<Item, Other, [...Depth, 0]> : false
  : NonNullable<A> extends object
    ? string extends Keys<NonNullable<A>> ? true
      : [Exclude<RequiredKeys<NonNullable<B>>, Keys<NonNullable<A>>>] extends [never]
        ? false extends { [K in Keys<NonNullable<A>> & Keys<NonNullable<B>>]: SameKeys<Field<NonNullable<A>, K>, Field<NonNullable<B>, K>, [...Depth, 0]> }[Keys<NonNullable<A>> & Keys<NonNullable<B>>] ? false : true
        : false
    : true;
type SchemaCheck<Url extends string, M extends Method, S extends z.ZodType> =
  [Extract<Url, `${string}?${string}`>] extends [never]
  ? [MissingOperation<Url, M>] extends [never]
    ? ShapeCheck<Response<Url, M>, S>
    : { readonly __unknown_api_operation: never }
  : { readonly __use_typed_query_options: never };

type ShapeCheck<Backend, S extends z.ZodType> = z.output<S> extends Backend
  ? SameKeys<Backend, z.output<S>> extends true
    ? WireShape<Backend, z.input<S>> extends true ? unknown : { readonly __response_wire_changed: never }
    : { readonly __response_fields_changed: never }
  : { readonly __response_contract_changed: never };

export type ApiSchema<Backend, S extends z.ZodType> = S & ShapeCheck<Backend, S>;

export type ContractSchema<Url extends string, M extends Method, S extends z.ZodType> = S & SchemaCheck<Url, M, S>;
export type ContractMethod = Method;
